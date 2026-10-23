//! Vanilla particle emission command.
//!
//! Recipient selection matches vanilla `ServerLevel.sendParticles`: every
//! online player is offered the packet and only players in the source world
//! within the limiter radius (32 blocks, or 512 with `force`) receive it.
//!
//! The `name` argument is provided by the shared
//! `Commands.CommandArguments` particle parser (`SteelArgumentType::particle`),
//! which resolves the registry type together with its options payload.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::particle_type::ParticleData;
use steel_utils::{Identifier, translations};
use text_components::TextComponent;

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError},
    execution::{
        CommandSource, SteelArgumentType, SteelCommandContext, SteelCommandRuntime, argument,
        literal,
    },
    registration::CommandRegistration,
};
use crate::player::Player;

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("particle"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("particle").then(
        argument("name", SteelArgumentType::particle())
            .executes(at_source)
            .then(
                argument("pos", SteelArgumentType::vec3(true))
                    .executes(at_position)
                    .then(
                        argument("delta", SteelArgumentType::vec3(false)).then(
                            argument("speed", ArgumentType::float(0.0, f32::MAX)).then(
                                argument("count", ArgumentType::integer(0, i32::MAX))
                                    .executes(normal_to_everyone)
                                    .then(
                                        literal("force").executes(force_to_everyone).then(
                                            argument("viewers", SteelArgumentType::players())
                                                .executes(force_to_viewers),
                                        ),
                                    )
                                    .then(
                                        literal("normal").executes(normal_to_everyone).then(
                                            argument("viewers", SteelArgumentType::players())
                                                .executes(normal_to_viewers),
                                        ),
                                    ),
                            ),
                        ),
                    ),
            ),
    )
}

/// `particle <name>` emits at the source position with no delta, speed, or count.
fn at_source(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let position = context.source().position();
    send_at(context, position, false, &all_players(context))
}

/// `particle <name> <pos>` emits at `pos` with no delta, speed, or count.
fn at_position(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let position = context.coordinates("pos")?.position(context.source());
    send_at(context, position, false, &all_players(context))
}

fn normal_to_everyone(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    send(context, false, &all_players(context))
}

fn force_to_everyone(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    send(context, true, &all_players(context))
}

fn force_to_viewers(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let viewers = context.players("viewers")?;
    send(context, true, &viewers)
}

fn normal_to_viewers(
    context: &SteelCommandContext<CommandSource>,
) -> Result<i32, CommandSyntaxError> {
    let viewers = context.players("viewers")?;
    send(context, false, &viewers)
}

fn send(
    context: &SteelCommandContext<CommandSource>,
    force: bool,
    players: &[Arc<Player>],
) -> Result<i32, CommandSyntaxError> {
    let position = context.coordinates("pos")?.position(context.source());
    send_at(context, position, force, players)
}

/// Emits one particle distribution and returns the number of recipients.
fn send_at(
    context: &SteelCommandContext<CommandSource>,
    position: DVec3,
    force: bool,
    players: &[Arc<Player>],
) -> Result<i32, CommandSyntaxError> {
    let particle = particle_data(context)?;
    let delta = context
        .coordinates("delta")
        .map_or(DVec3::ZERO, |delta| delta.position(context.source()));
    let speed = context.float("speed").unwrap_or(0.0);
    let count = context.integer("count").unwrap_or(0);

    let world = context.source().world();
    let mut sent = 0i32;
    for player in players {
        if world.send_particles_to(
            player,
            particle.clone(),
            force,
            false,
            position,
            count,
            delta,
            f64::from(speed),
        ) {
            sent += 1;
        }
    }

    if sent == 0 {
        return Err(CommandSyntaxError::dynamic(TextComponent::from(
            &translations::COMMANDS_PARTICLE_FAILED,
        )));
    }

    let message = translations::COMMANDS_PARTICLE_SUCCESS
        .message([particle.particle_type().key.to_string()])
        .component();
    context.source().send_success(&message, true);
    Ok(sent)
}

/// Resolves the particle argument to a sendable value.
///
/// Steel's `ParticleArgument` currently parses only the bare type key, so the
/// payload is the type's zero-argument options. Types whose payload requires
/// fields (`minecraft:dust`, `minecraft:block`, …) are rejected until the
/// argument learns vanilla's `{options}` syntax.
fn particle_data(
    context: &SteelCommandContext<CommandSource>,
) -> Result<ParticleData, CommandSyntaxError> {
    let particle_type = context.particle("name")?;
    ParticleData::with_default_options(particle_type).ok_or_else(|| {
        CommandSyntaxError::dynamic(format!(
            "Particle '{}' requires options, which are not supported yet",
            particle_type.key
        ))
    })
}

/// Vanilla uses `server.getPlayerList().getPlayers()` for the "no viewers"
/// form; every player is offered the packet and the world-level recipient
/// filter decides delivery, so the source world's player map is equivalent.
fn all_players(context: &SteelCommandContext<CommandSource>) -> Vec<Arc<Player>> {
    let mut players = Vec::new();
    context.source().world().players.iter_players(|_, player| {
        players.push(Arc::clone(player));
        true
    });
    players
}
