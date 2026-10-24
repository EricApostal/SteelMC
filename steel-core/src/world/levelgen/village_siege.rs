//! Vanilla `VillageSiege`: after the `roll_village_siege` clock marker during a
//! night, a village may be attacked by a wave of zombies spawned in its ring.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::REGISTRY;
use steel_registry::vanilla_biome_tags::BiomeTag;
use steel_registry::vanilla_entities;
use steel_registry::world_clock::WorldClockRef;
use steel_utils::BlockPos;
use steel_utils::random::{Random as _, RandomSource};

use crate::chunk::heightmap::HeightmapType;
use crate::entity::Entity as _;
use crate::entity::{EntitySpawnReason, create_entity_instance};
use crate::world::World;
use crate::world::custom_spawner::CustomSpawner;
use crate::world::spawn::natural::runtime_random;

/// Vanilla `VillageSiege.State`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SiegeState {
    CanActivate,
    Tonight,
    Done,
}

/// Vanilla `VillageSiege`.
pub struct VillageSiege {
    has_setup_siege: bool,
    siege_state: SiegeState,
    zombies_to_spawn: i32,
    next_spawn_time: i32,
    spawn_x: i32,
    spawn_y: i32,
    spawn_z: i32,
}

impl Default for VillageSiege {
    fn default() -> Self {
        Self::new()
    }
}

impl VillageSiege {
    /// Creates the siege with vanilla's initial state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            has_setup_siege: false,
            siege_state: SiegeState::Done,
            zombies_to_spawn: 0,
            next_spawn_time: 0,
            spawn_x: 0,
            spawn_y: 0,
            spawn_z: 0,
        }
    }

    /// Vanilla `VillageSiege.tick`.
    pub fn tick_with_settings(&mut self, world: &Arc<World>, spawn_enemies: bool) {
        if world.is_bright_outside() || !spawn_enemies {
            self.siege_state = SiegeState::Done;
            self.has_setup_siege = false;
            return;
        }

        let Some(default_clock) = world.dimension_type.default_clock else {
            return;
        };

        let mut random = runtime_random();
        if is_at_time_marker(world, default_clock, &roll_village_siege_marker()) {
            self.siege_state = if random.next_i32_bounded(10) == 0 {
                SiegeState::Tonight
            } else {
                SiegeState::Done
            };
        }

        if self.siege_state == SiegeState::Done {
            return;
        }

        if !self.has_setup_siege {
            if !self.try_to_setup_siege(world, &mut random) {
                return;
            }
            self.has_setup_siege = true;
        }

        if self.next_spawn_time > 0 {
            self.next_spawn_time -= 1;
            return;
        }

        self.next_spawn_time = 2;
        if self.zombies_to_spawn > 0 {
            self.try_spawn(world);
            self.zombies_to_spawn -= 1;
        } else {
            self.siege_state = SiegeState::Done;
        }
    }

    /// Vanilla `VillageSiege.tryToSetupSiege`.
    fn try_to_setup_siege(&mut self, world: &Arc<World>, random: &mut RandomSource) -> bool {
        let mut players = Vec::new();
        world.players.iter_players(|_, player| {
            players.push(Arc::clone(player));
            true
        });

        for player in players {
            if player.is_spectator() {
                continue;
            }

            let center = player.block_position();
            if !world.is_village(center)
                || world.biome_at(center).is_some_and(|biome| {
                    steel_registry::biome::Biome::has_tag(biome, &BiomeTag::WITHOUT_ZOMBIE_SIEGES)
                })
            {
                continue;
            }

            for _ in 0..10 {
                let angle = random.next_f32() * std::f32::consts::TAU;
                self.spawn_x = center.x() + (angle.cos() * 32.0) as i32;
                self.spawn_y = center.y();
                self.spawn_z = center.z() + (angle.sin() * 32.0) as i32;
                if self
                    .find_random_spawn_pos(
                        world,
                        BlockPos::new(self.spawn_x, self.spawn_y, self.spawn_z),
                        random,
                    )
                    .is_some()
                {
                    self.next_spawn_time = 0;
                    self.zombies_to_spawn = 20;
                    break;
                }
            }

            return true;
        }

        false
    }

    /// Vanilla `VillageSiege.trySpawn`.
    fn try_spawn(&self, world: &Arc<World>) {
        let mut random = runtime_random();
        let Some(spawn_pos) = self.find_random_spawn_pos(
            world,
            BlockPos::new(self.spawn_x, self.spawn_y, self.spawn_z),
            &mut random,
        ) else {
            return;
        };

        let Ok(zombie) = create_entity_instance(world, &vanilla_entities::ZOMBIE, spawn_pos) else {
            return;
        };

        if let Some(mob) = zombie.as_mob() {
            mob.finalize_spawn(world, EntitySpawnReason::Event, None);
        }
        zombie.snap_to(spawn_pos, random.next_f32() * 360.0, 0.0);
        let _ = world.try_add_entity(zombie);
    }

    /// Vanilla `VillageSiege.findRandomSpawnPos`.
    fn find_random_spawn_pos(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        random: &mut RandomSource,
    ) -> Option<DVec3> {
        for _ in 0..10 {
            let x = pos.x() + random.next_i32_bounded(16) - 8;
            let z = pos.z() + random.next_i32_bounded(16) - 8;
            let y = world.level_height_at(HeightmapType::WorldSurface, x, z);
            let candidate = BlockPos::new(x, y, z);
            if !world.is_village(candidate) {
                continue;
            }

            if crate::world::spawn::placement::SpawnPlacements::check_spawn_rules(
                &vanilla_entities::ZOMBIE,
                world,
                EntitySpawnReason::Event,
                candidate,
                random,
            ) {
                return Some(DVec3::new(
                    f64::from(candidate.x()) + 0.5,
                    f64::from(candidate.y()),
                    f64::from(candidate.z()) + 0.5,
                ));
            }
        }

        None
    }
}

impl CustomSpawner for VillageSiege {
    fn tick(&mut self, world: &Arc<World>) {
        self.tick_with_settings(world, world.is_spawning_monsters());
    }

    fn tick_with_settings(&mut self, world: &Arc<World>, spawn_enemies: bool) {
        Self::tick_with_settings(self, world, spawn_enemies);
    }
}

/// Vanilla `ClockTimeMarkers.ROLL_VILLAGE_SIEGE`.
fn roll_village_siege_marker() -> steel_utils::Identifier {
    steel_utils::Identifier::vanilla_static("roll_village_siege")
}

/// Vanilla `ServerClockManager.isAtTimeMarker`.
fn is_at_time_marker(
    world: &World,
    clock: WorldClockRef,
    marker: &steel_utils::Identifier,
) -> bool {
    let Some(total_ticks) = world.clock_total_ticks(clock) else {
        return false;
    };

    REGISTRY.timelines.iter().any(|(_, timeline)| {
        std::ptr::eq(timeline.clock, clock)
            && timeline.time_markers.iter().any(|time_marker| {
                time_marker.key == *marker
                    && timeline
                        .period_ticks
                        .map_or(i64::from(time_marker.ticks) == total_ticks, |period| {
                            i64::from(time_marker.ticks) == total_ticks % i64::from(period)
                        })
            })
    })
}
