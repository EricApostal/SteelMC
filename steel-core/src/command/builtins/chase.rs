//! Vanilla chase command.
//!
//! Vanilla `ChaseCommand` mirrors one server's first player onto another
//! server over a plain-text TCP protocol: `/chase lead` broadcasts
//! `t <dim> x y z yaw pitch` every 100 ms, and `/chase follow` teleports its
//! own first player with `execute in <dimension> run tp @s …`.

use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use steel_utils::Identifier;
use steel_utils::locks::SyncMutex;
use text_components::TextComponent;

use super::super::{
    brigadier::{ArgumentType, CommandNodeBuilder, CommandSyntaxError},
    execution::{CommandSource, SteelCommandContext, SteelCommandRuntime, argument, literal},
    registration::CommandRegistration,
};
use crate::command::sender::CommandSender;
use crate::entity::Entity;
use crate::server::Server;

/// Vanilla `ChaseCommand.DEFAULT_CONNECT_HOST`.
const DEFAULT_CONNECT_HOST: &str = "localhost";
/// Vanilla `ChaseCommand.DEFAULT_BIND_ADDRESS`.
const DEFAULT_BIND_ADDRESS: &str = "0.0.0.0";
/// Vanilla `ChaseCommand.DEFAULT_PORT`.
const DEFAULT_PORT: i32 = 10000;
/// Vanilla `ChaseCommand.BROADCAST_INTERVAL_MS`.
const BROADCAST_INTERVAL_MS: u64 = 100;

/// Vanilla `ChaseClient.RECONNECT_INTERVAL_SECONDS`.
const RECONNECT_INTERVAL: Duration = Duration::from_secs(5);

/// A running chase endpoint that `/chase stop` can shut down.
struct Session {
    /// Set to stop the worker thread.
    shutdown: Arc<AtomicBool>,
    /// Client-side socket, shut down to unblock the reader.
    socket: Arc<SyncMutex<Option<TcpStream>>>,
}

impl Session {
    fn stop(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(socket) = self.socket.lock().take() {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
}

/// Vanilla `ChaseCommand`'s static `chaseServer`/`chaseClient` pair.
#[derive(Default)]
struct ChaseState {
    /// The followed server, vanilla `chaseClient`.
    client: Option<Session>,
    /// The served endpoint, vanilla `chaseServer`.
    server: Option<Session>,
}

/// The process-wide chase endpoints.
static CHASE: SyncMutex<ChaseState> = SyncMutex::new(ChaseState {
    client: None,
    server: None,
});

/// Vanilla `ChaseServer.PlayerPosition`.
#[derive(Debug, Clone, PartialEq)]
struct PlayerPosition {
    /// The chase dimension letter: `o`, `n`, or `e`.
    dimension: &'static str,
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
    pitch: f32,
}

impl PlayerPosition {
    /// Vanilla `ChaseServer.PlayerPosition.format`.
    fn format(&self) -> String {
        format!(
            "t {} {:.2} {:.2} {:.2} {:.2} {:.2}\n",
            self.dimension, self.x, self.y, self.z, self.yaw, self.pitch
        )
    }
}

pub(super) fn registration() -> CommandRegistration<CommandSource> {
    CommandRegistration::new(Identifier::vanilla_static("chase"), |_| command())
}

fn command() -> CommandNodeBuilder<CommandSource, SteelCommandRuntime> {
    literal("chase")
        .then(
            literal("follow")
                .then(
                    argument("host", ArgumentType::string())
                        .executes(|context| follow(context, DEFAULT_CONNECT_HOST, DEFAULT_PORT))
                        .then(
                            argument("port", ArgumentType::integer(1, 65535))
                                .executes(|context| follow_port(context)),
                        ),
                )
                .executes(|context| follow(context, DEFAULT_CONNECT_HOST, DEFAULT_PORT)),
        )
        .then(
            literal("lead")
                .then(
                    argument("bind_address", ArgumentType::string())
                        .executes(|context| lead(context, DEFAULT_BIND_ADDRESS, DEFAULT_PORT))
                        .then(
                            argument("port", ArgumentType::integer(1024, 65535))
                                .executes(|context| lead_port(context)),
                        ),
                )
                .executes(|context| lead(context, DEFAULT_BIND_ADDRESS, DEFAULT_PORT)),
        )
        .then(literal("stop").executes(stop_chase))
}

/// Vanilla `ChaseCommand.follow` with the parsed `host` and `port`.
fn follow_port(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let host = context.string("host")?.to_owned();
    let port = context.integer("port")?;
    follow(context, &host, port)
}

/// Vanilla `ChaseCommand.lead` with the parsed bind address and `port`.
fn lead_port(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let address = context.string("bind_address")?.to_owned();
    let port = context.integer("port")?;
    lead(context, &address, port)
}

/// Vanilla `ChaseCommand.alreadyRunning`.
fn already_running(context: &SteelCommandContext<CommandSource>) -> bool {
    let (server_running, client_running) = {
        let state = CHASE.lock();
        (state.server.is_some(), state.client.is_some())
    };
    if server_running {
        context.source().send_failure(TextComponent::plain(
            "Chase server is already running. Stop it using /chase stop",
        ));
        return true;
    }
    if client_running {
        context.source().send_failure(TextComponent::plain(
            "You are already chasing someone. Stop it using /chase stop",
        ));
        return true;
    }
    false
}

/// Vanilla `ChaseCommand.lead`.
#[expect(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    reason = "the command grammar already bounds the port to 1024..=65535"
)]
fn lead(
    context: &SteelCommandContext<CommandSource>,
    bind_address: &str,
    port: i32,
) -> Result<i32, CommandSyntaxError> {
    if already_running(context) {
        return Ok(0);
    }

    let listener = match TcpListener::bind((bind_address, port as u16)) {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(%error, port, "failed to start chase server");
            context.source().send_failure(TextComponent::plain(format!(
                "Failed to start chase server on port {port}"
            )));
            return Ok(0);
        }
    };
    let _ = listener.set_nonblocking(true);

    let shutdown = Arc::new(AtomicBool::new(false));
    let server = Arc::clone(context.source().server());
    let worker_shutdown = Arc::clone(&shutdown);
    let spawn_result = std::thread::Builder::new()
        .name("chase-server".to_owned())
        .spawn(move || run_server(listener, &server, &worker_shutdown));
    if let Err(error) = spawn_result {
        tracing::error!(%error, port, "failed to start chase server thread");
        context.source().send_failure(TextComponent::plain(format!(
            "Failed to start chase server on port {port}"
        )));
        return Ok(0);
    }

    CHASE.lock().server = Some(Session {
        shutdown,
        socket: Arc::new(SyncMutex::new(None)),
    });
    context.source().send_success(
        &TextComponent::plain(format!(
            "Chase server is now running on port {port}. Clients can follow you using /chase follow <ip> <port>"
        )),
        false,
    );
    Ok(0)
}

/// Vanilla `ChaseCommand.follow`.
#[expect(
    clippy::cast_sign_loss,
    clippy::cast_possible_truncation,
    reason = "the command grammar already bounds the port to 1..=65535"
)]
fn follow(
    context: &SteelCommandContext<CommandSource>,
    host: &str,
    port: i32,
) -> Result<i32, CommandSyntaxError> {
    if already_running(context) {
        return Ok(0);
    }

    let shutdown = Arc::new(AtomicBool::new(false));
    let socket = Arc::new(SyncMutex::new(None));
    let server = Arc::clone(context.source().server());
    let worker_shutdown = Arc::clone(&shutdown);
    let worker_socket = Arc::clone(&socket);
    let address = format!("{host}:{port}");
    let client_address = address.clone();
    let spawn_result = std::thread::Builder::new()
        .name("chase-client".to_owned())
        .spawn(move || run_client(&client_address, &server, &worker_shutdown, &worker_socket));
    if let Err(error) = spawn_result {
        tracing::error!(%error, %address, "failed to start chase client thread");
        context.source().send_failure(TextComponent::plain(format!(
            "Failed to start chase client for {address}"
        )));
        return Ok(0);
    }

    CHASE.lock().client = Some(Session { shutdown, socket });
    context.source().send_success(
        &TextComponent::plain(format!(
            "You are now chasing {host}:{port}. If that server does '/chase lead' then you will automatically go to the same position. Use '/chase stop' to stop chasing."
        )),
        false,
    );
    Ok(0)
}

/// Vanilla `ChaseCommand.stop`.
fn stop_chase(context: &SteelCommandContext<CommandSource>) -> Result<i32, CommandSyntaxError> {
    let (client, server) = {
        let mut state = CHASE.lock();
        (state.client.take(), state.server.take())
    };

    if let Some(client) = client {
        client.stop();
        context
            .source()
            .send_success(&TextComponent::plain("You have now stopped chasing"), false);
    }
    if let Some(server) = server {
        server.stop();
        context.source().send_success(
            &TextComponent::plain("You are no longer being chased"),
            false,
        );
    }
    Ok(0)
}

/// Vanilla `ChaseServer`: accept followers and broadcast the first player.
fn run_server(listener: TcpListener, server: &Arc<Server>, shutdown: &AtomicBool) {
    let mut clients: Vec<TcpStream> = Vec::new();
    let mut last_position: Option<PlayerPosition> = None;

    while !shutdown.load(Ordering::Relaxed) {
        while let Ok((stream, address)) = listener.accept() {
            tracing::info!(%address, "chase client connected");
            let _ = stream.set_nodelay(true);
            clients.push(stream);
        }

        if !clients.is_empty()
            && let Some(position) = first_player_position(server)
            && last_position.as_ref() != Some(&position)
        {
            let line = position.format();
            clients.retain_mut(|client| {
                client
                    .write_all(line.as_bytes())
                    .and_then(|()| client.flush())
                    .is_ok()
            });
            last_position = Some(position);
        }

        std::thread::sleep(Duration::from_millis(BROADCAST_INTERVAL_MS));
    }

    tracing::info!("chase server stopped");
}

/// Vanilla `ChaseServer.getPlayerPosition`.
fn first_player_position(server: &Arc<Server>) -> Option<PlayerPosition> {
    let players = server.get_players();
    let player = players.first()?;
    let world = player.get_world();
    let dimension = dimension_letter(&world.key)?;
    let position = player.position();
    let (yaw, pitch) = player.rotation();
    Some(PlayerPosition {
        dimension,
        x: position.x,
        y: position.y,
        z: position.z,
        yaw,
        pitch,
    })
}

/// Vanilla `ChaseClient`: follow a remote server's broadcast teleports.
fn run_client(
    address: &str,
    server: &Arc<Server>,
    shutdown: &AtomicBool,
    socket: &SyncMutex<Option<TcpStream>>,
) {
    while !shutdown.load(Ordering::Relaxed) {
        tracing::info!(%address, "connecting to chase server");
        match TcpStream::connect(address) {
            Ok(stream) => {
                if let Ok(reader_stream) = stream.try_clone() {
                    *socket.lock() = Some(stream);
                    let mut reader = BufReader::new(reader_stream);
                    let mut line = String::new();
                    loop {
                        if shutdown.load(Ordering::Relaxed) {
                            return;
                        }
                        line.clear();
                        match reader.read_line(&mut line) {
                            Ok(0) => {
                                tracing::warn!(
                                    %address,
                                    "lost connection to chase server; retrying in 5s"
                                );
                                break;
                            }
                            Ok(_) => handle_message(server, line.trim_end()),
                            Err(error) => {
                                if !shutdown.load(Ordering::Relaxed) {
                                    tracing::warn!(
                                        %error, %address,
                                        "lost connection to chase server; retrying in 5s"
                                    );
                                }
                                break;
                            }
                        }
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, %address, "failed to connect to chase server; retrying in 5s");
            }
        }

        if let Some(stream) = socket.lock().take() {
            let _ = stream.shutdown(Shutdown::Both);
        }

        let deadline = Instant::now() + RECONNECT_INTERVAL;
        while Instant::now() < deadline && !shutdown.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(BROADCAST_INTERVAL_MS));
        }
    }
}

/// Vanilla `ChaseClient.handleMessage`.
fn handle_message(server: &Arc<Server>, message: &str) {
    let mut parts = message.split_ascii_whitespace();
    match parts.next() {
        Some("t") => handle_teleport(server, &mut parts),
        Some(other) => tracing::warn!(kind = other, "unknown chase message type"),
        None => tracing::warn!("could not parse empty chase message"),
    }
}

/// Vanilla `ChaseClient.handleTeleport`.
fn handle_teleport<'a>(server: &Arc<Server>, parts: &mut impl Iterator<Item = &'a str>) {
    let Some(dimension) = parts.next() else {
        tracing::warn!("could not parse chase teleport message");
        return;
    };
    let Some(level) = dimension_identifier(dimension) else {
        return;
    };

    let mut values = [0.0_f32; 5];
    for value in &mut values {
        let Some(parsed) = parts.next().and_then(|part| part.parse::<f32>().ok()) else {
            tracing::warn!("could not parse chase teleport message");
            return;
        };
        *value = parsed;
    }
    let [x, y, z, yaw, pitch] = values;

    let command = format!("execute in {level} run tp @s {x:.3} {y:.3} {z:.3} {yaw:.3} {pitch:.3}");
    let Some(player) = server.get_players().first().cloned() else {
        return;
    };
    let _ = server.submit_command(CommandSender::Player(player), command);
}

/// Vanilla `ChaseCommand.DIMENSION_NAMES` inverse lookup.
fn dimension_letter(key: &Identifier) -> Option<&'static str> {
    match key.path.as_ref() {
        "overworld" => Some("o"),
        "the_nether" => Some("n"),
        "the_end" => Some("e"),
        _ => None,
    }
}

/// Vanilla `ChaseCommand.DIMENSION_NAMES` forward lookup.
fn dimension_identifier(letter: &str) -> Option<Identifier> {
    match letter {
        "o" => Some(Identifier::vanilla_static("overworld")),
        "n" => Some(Identifier::vanilla_static("the_nether")),
        "e" => Some(Identifier::vanilla_static("the_end")),
        _ => None,
    }
}
