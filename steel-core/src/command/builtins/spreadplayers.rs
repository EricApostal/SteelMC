//! Vanilla player/entity spreading command.

use std::sync::Arc;

use glam::DVec3;
use steel_protocol::packets::game::RelativeMovement;
use steel_registry::{blocks::block_state_ext::BlockStateExt as _, vanilla_block_tags::BlockTag};
use steel_utils::{
    BlockPos, BlockStateId, Identifier,
    random::{Random as _, RandomSource, legacy_random::LegacyRandom},
    translations,
};
use text_components::TextComponent;

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use crate::{
    entity::{SharedEntity, change_entity_world},
    portal::{TeleportPostTransition, TeleportTransition},
    scoreboard::ScoreHolder,
    world::World,
};

type Builder = CommandNodeBuilder<CommandSource, SteelCommandRuntime>;

/// Vanilla `SpreadPlayersCommand.MAX_ITERATION_COUNT`.
const MAX_ITERATION_COUNT: i32 = 10_000;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("spreadplayers"), |_| command())
}

fn command() -> Builder {
    literal("spreadplayers").then(
        argument("center", SteelArgumentType::vec2(false)).then(
            argument("spreadDistance", ArgumentType::float(0.0, f32::MAX)).then(
                argument("maxRange", ArgumentType::float(1.0, f32::MAX))
                    .then(argument("respectTeams", ArgumentType::bool()).then(
                        argument("targets", SteelArgumentType::entities()).executes(
                            |context: &SteelCommandContext<CommandSource>| {
                                let max_height = context.source().world().get_max_y() + 1;
                                execute_spread(context, max_height)
                            },
                        ),
                    ))
                    .then(literal("under").then(
                        argument("maxHeight", ArgumentType::integer(i32::MIN, i32::MAX)).then(
                            argument("respectTeams", ArgumentType::bool()).then(
                                argument("targets", SteelArgumentType::entities()).executes(
                                    |context| {
                                        let max_height = context.integer("maxHeight")?;
                                        execute_spread(context, max_height)
                                    },
                                ),
                            ),
                        ),
                    )),
            ),
        ),
    )
}

fn execute_spread(
    context: &SteelCommandContext<CommandSource>,
    max_height: i32,
) -> Result<i32, CommandSyntaxError> {
    let source = context.source();
    let world = Arc::clone(source.world());
    let min_y = world.get_min_y();
    if max_height < min_y {
        let message = translations::COMMANDS_SPREADPLAYERS_FAILED_INVALID_HEIGHT
            .message([
                TextComponent::from(max_height.to_string()),
                TextComponent::from(min_y.to_string()),
            ])
            .component();
        return Err(CommandSyntaxError::dynamic(message));
    }

    let center = context.coordinates("center")?.position(source);
    let spread_distance = context.float("spreadDistance")?;
    let max_range = context.float("maxRange")?;
    let respect_teams = context.boolean("respectTeams")?;
    let entities = context.entities("targets")?;

    let (message, count) = spread_players(
        &world,
        center,
        spread_distance,
        f64::from(max_range),
        max_height,
        respect_teams,
        &entities,
    )?;
    source.send_success(&message, true);
    Ok(count)
}

fn spread_players(
    world: &Arc<World>,
    center: DVec3,
    spread_distance: f32,
    max_range: f64,
    max_height: i32,
    respect_teams: bool,
    entities: &[SharedEntity],
) -> Result<(TextComponent, i32), CommandSyntaxError> {
    let mut random = RandomSource::Legacy(LegacyRandom::from_seed(rand::random::<u64>()));
    let min_x = center.x - max_range;
    let min_z = center.z - max_range;
    let max_x = center.x + max_range;
    let max_z = center.z + max_range;
    let position_count = if respect_teams {
        count_teams(world, entities)
    } else {
        entities.len()
    };
    let mut positions =
        create_initial_positions(&mut random, position_count, min_x, min_z, max_x, max_z);
    spread_positions(
        &mut random,
        world,
        &mut positions,
        (min_x, min_z, max_x, max_z),
        max_height,
        f64::from(spread_distance),
        respect_teams,
    )?;
    let distance = set_player_positions(world, entities, &positions, max_height, respect_teams);

    let translation = if respect_teams {
        &translations::COMMANDS_SPREADPLAYERS_SUCCESS_TEAMS
    } else {
        &translations::COMMANDS_SPREADPLAYERS_SUCCESS_ENTITIES
    };
    let message = translation
        .message([
            TextComponent::from(positions.len().to_string()),
            TextComponent::from(format!("{}", center.x)),
            TextComponent::from(format!("{}", center.z)),
            TextComponent::from(format!("{distance:.2}")),
        ])
        .component();
    Ok((message, i32::try_from(positions.len()).unwrap_or(i32::MAX)))
}

/// Vanilla `SpreadPlayersCommand.getNumberOfTeams`.
fn count_teams(world: &World, entities: &[SharedEntity]) -> usize {
    let mut teams: Vec<Option<String>> = Vec::new();
    for entity in entities {
        let team = player_team_name(world, entity);
        if !teams.contains(&team) {
            teams.push(team);
        }
    }
    teams.len()
}

/// Vanilla `Entity instanceof Player ? entity.getTeam() : null`, resolved through
/// the domain scoreboard's holder membership.
fn player_team_name(world: &World, entity: &SharedEntity) -> Option<String> {
    let player = entity.as_player()?;
    let server = player.server();
    let scoreboard = server.scoreboards.get(world.domain())?;
    scoreboard.holder_team_name(&ScoreHolder::new(player.gameprofile.name()))
}

/// One candidate spread position.
#[derive(Clone, Copy, Default)]
struct Position {
    x: f64,
    z: f64,
}

impl Position {
    fn dist(self, other: Self) -> f64 {
        let dx = self.x - other.x;
        let dz = self.z - other.z;
        (dx * dx + dz * dz).sqrt()
    }

    fn clamp(&mut self, min_x: f64, min_z: f64, max_x: f64, max_z: f64) -> bool {
        let mut changed = false;
        if self.x < min_x {
            self.x = min_x;
            changed = true;
        } else if self.x > max_x {
            self.x = max_x;
            changed = true;
        }
        if self.z < min_z {
            self.z = min_z;
            changed = true;
        } else if self.z > max_z {
            self.z = max_z;
            changed = true;
        }
        changed
    }

    /// Vanilla `Position.getSpawnY`.
    fn spawn_y(self, world: &World, max_height: i32) -> i32 {
        let x = floor_i32(self.x);
        let z = floor_i32(self.z);
        let mut y = max_height + 1;
        let mut air_2_above = world.get_block_state(BlockPos::new(x, y, z)).is_air();
        y -= 1;
        let mut air_1_above = world.get_block_state(BlockPos::new(x, y, z)).is_air();
        while y > world.get_min_y() {
            y -= 1;
            let current_is_air = world.get_block_state(BlockPos::new(x, y, z)).is_air();
            if !current_is_air && air_1_above && air_2_above {
                return y + 1;
            }
            air_2_above = air_1_above;
            air_1_above = current_is_air;
        }
        max_height + 1
    }

    /// Vanilla `Position.isSafe`.
    fn is_safe(self, world: &World, max_height: i32) -> bool {
        let pos = BlockPos::new(
            floor_i32(self.x),
            self.spawn_y(world, max_height) - 1,
            floor_i32(self.z),
        );
        let state = world.get_block_state(pos);
        pos.y() < max_height
            && !is_liquid(state)
            && state
                .get_block()
                .has_tag(&BlockTag::ENTITIES_CAN_TELEPORT_TO)
    }

    /// Vanilla `Position.randomize`.
    fn randomize(
        &mut self,
        random: &mut RandomSource,
        min_x: f64,
        min_z: f64,
        max_x: f64,
        max_z: f64,
    ) {
        self.x = next_double(random, min_x, max_x);
        self.z = next_double(random, min_z, max_z);
    }
}

/// Vanilla `BlockState.liquid()`.
fn is_liquid(state: BlockStateId) -> bool {
    state.get_block().config.liquid
}

/// Vanilla `Mth.nextDouble`: `min >= max` returns `min`.
fn next_double(random: &mut RandomSource, min: f64, max: f64) -> f64 {
    if min >= max {
        min
    } else {
        random.next_f64() * (max - min) + min
    }
}

/// Vanilla `Mth.floor`.
const fn floor_i32(value: f64) -> i32 {
    value.floor() as i32
}

fn create_initial_positions(
    random: &mut RandomSource,
    count: usize,
    min_x: f64,
    min_z: f64,
    max_x: f64,
    max_z: f64,
) -> Vec<Position> {
    let mut result = Vec::with_capacity(count);
    for _ in 0..count {
        let mut position = Position::default();
        position.randomize(random, min_x, min_z, max_x, max_z);
        result.push(position);
    }
    result
}

/// Vanilla `SpreadPlayersCommand.spreadPositions`.
fn spread_positions(
    random: &mut RandomSource,
    world: &World,
    positions: &mut [Position],
    bounds: (f64, f64, f64, f64),
    max_height: i32,
    spread_distance: f64,
    respect_teams: bool,
) -> Result<(), CommandSyntaxError> {
    let (min_x, min_z, max_x, max_z) = bounds;
    let mut has_collisions = true;
    let mut min_distance: Option<f64> = None;
    let mut iteration = 0;
    while iteration < MAX_ITERATION_COUNT && has_collisions {
        has_collisions = false;
        min_distance = None;

        for i in 0..positions.len() {
            let mut neighbour_count = 0;
            let mut average_x = 0.0;
            let mut average_z = 0.0;
            let position = positions[i];
            for (j, neighbour) in positions.iter().copied().enumerate() {
                if i == j {
                    continue;
                }
                let dist = position.dist(neighbour);
                let closest = min_distance.unwrap_or(f64::MAX);
                min_distance = Some(closest.min(dist));
                if dist < spread_distance {
                    neighbour_count += 1;
                    average_x += neighbour.x - position.x;
                    average_z += neighbour.z - position.z;
                }
            }

            if neighbour_count > 0 {
                average_x /= f64::from(neighbour_count);
                average_z /= f64::from(neighbour_count);
                let length = (average_x * average_x + average_z * average_z).sqrt();
                if length > 0.0 {
                    positions[i].x -= average_x / length;
                    positions[i].z -= average_z / length;
                } else {
                    positions[i].randomize(random, min_x, min_z, max_x, max_z);
                }
                has_collisions = true;
            }

            if positions[i].clamp(min_x, min_z, max_x, max_z) {
                has_collisions = true;
            }
        }

        if !has_collisions {
            for position in positions.iter_mut() {
                if !position.is_safe(world, max_height) {
                    position.randomize(random, min_x, min_z, max_x, max_z);
                    has_collisions = true;
                }
            }
        }
        iteration += 1;
    }

    // Vanilla reports `0.0` when no pair was measured (fewer than two entities).
    let min_distance = min_distance.unwrap_or(0.0);

    if iteration >= MAX_ITERATION_COUNT {
        let translation = if respect_teams {
            &translations::COMMANDS_SPREADPLAYERS_FAILED_TEAMS
        } else {
            &translations::COMMANDS_SPREADPLAYERS_FAILED_ENTITIES
        };
        let message = translation
            .message([
                TextComponent::from(positions.len().to_string()),
                TextComponent::from(format!("{min_x}")),
                TextComponent::from(format!("{min_z}")),
                TextComponent::from(format!("{min_distance:.2}")),
            ])
            .component();
        return Err(CommandSyntaxError::dynamic(message));
    }

    Ok(())
}

/// Vanilla `SpreadPlayersCommand.setPlayerPositions`.
fn set_player_positions(
    world: &Arc<World>,
    entities: &[SharedEntity],
    positions: &[Position],
    max_height: i32,
    respect_teams: bool,
) -> f64 {
    let mut total_distance = 0.0;
    let mut position_index = 0;
    let mut team_positions: Vec<(Option<String>, usize)> = Vec::new();

    for entity in entities {
        let position = if respect_teams {
            let team = player_team_name(world, entity);
            let existing = team_positions
                .iter()
                .find(|(name, _)| *name == team)
                .map(|(_, index)| *index);
            if let Some(index) = existing {
                positions[index]
            } else {
                let index = position_index;
                position_index += 1;
                team_positions.push((team, index));
                positions[index]
            }
        } else {
            let index = position_index;
            position_index += 1;
            positions[index]
        };

        let transition = TeleportTransition {
            target_world: Arc::clone(world),
            position: DVec3::new(
                f64::from(floor_i32(position.x)) + 0.5,
                f64::from(position.spawn_y(world, max_height)),
                f64::from(floor_i32(position.z)) + 0.5,
            ),
            rotation: entity.rotation(),
            velocity: DVec3::ZERO,
            relatives: RelativeMovement::NONE,
            portal_cooldown: 0,
            as_passenger: false,
            post_transition: TeleportPostTransition::do_nothing(),
        };
        change_entity_world(Arc::clone(entity), &transition);

        let mut closest = f64::MAX;
        for test in positions {
            // Vanilla skips candidates sitting exactly on the entity's column.
            #[expect(clippy::float_cmp, reason = "vanilla compares exactly")]
            let same_column = test.x == position.x && test.z == position.z;
            if !same_column {
                closest = closest.min(position.dist(*test));
            }
        }
        total_distance += closest;
    }

    if entities.len() < 2 {
        0.0
    } else {
        total_distance / entities.len() as f64
    }
}
