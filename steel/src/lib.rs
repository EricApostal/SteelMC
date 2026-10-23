//! # Steel
//!
//! The main library for the Steel Minecraft server.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::{
    error::Error,
    fmt, io,
    net::{Ipv4Addr, SocketAddrV4},
    sync::{Arc, OnceLock},
};

use steel_core::{
    GIT_HASH_SHORT, command::CommandRegistry, permission::PermissionGroupManager, server::Server,
};
use steel_login::{JavaTcpClient, ServerConnectionSession};
use tokio::{net::TcpListener, runtime::Runtime, select};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

/// Server configuration module.
pub mod config;
/// A module for logging utilities.
pub mod logger;
/// Save-root locking (`session.lock`), mirroring vanilla's `DirectoryLock`.
pub mod session_lock;

/// Static access to the server
pub static SERVER: OnceLock<Arc<Server>> = OnceLock::new();

/// The main server struct.
pub struct SteelServer {
    /// The TCP listener for incoming connections.
    ///
    /// `None` after shutdown closes it, so no new connection is accepted while
    /// player and world data is being saved (vanilla `ServerConnection#stop`).
    pub tcp_listener: Option<TcpListener>,
    /// The cancellation token for graceful shutdown.
    pub cancel_token: CancellationToken,
    /// The next client ID to be assigned.
    pub client_id: u64,
    /// The shared server state.
    pub server: Arc<Server>,
    /// Session id UUID state
    pub connection_session: Arc<ServerConnectionSession>,
    /// Exclusive lock on the save root, released once this value is dropped.
    pub session_lock: session_lock::SessionLock,
}

/// Startup error for expected operational failures.
#[derive(Debug)]
pub enum SteelServerError {
    /// Core server startup failed.
    Core(String),
    /// TCP listener could not bind.
    Bind {
        /// Server port that failed to bind.
        port: u16,
        /// Underlying IO error.
        source: io::Error,
    },
    /// Another server instance already holds the save root.
    SaveLocked {
        /// Lock file that could not be acquired.
        path: PathBuf,
        /// Underlying IO error.
        source: io::Error,
    },
}

impl fmt::Display for SteelServerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core(error) => f.write_str(error),
            Self::Bind { port, source } => {
                write!(f, "failed to bind to server port {port}: {source}")
            }
            Self::SaveLocked { path, source } => {
                write!(f, "failed to lock {}: {source}", path.display())
            }
        }
    }
}

impl Error for SteelServerError {}

impl SteelServer {
    /// Creates a new Steel server.
    ///
    pub async fn new(
        chunk_runtime: Arc<Runtime>,
        cancel_token: CancellationToken,
        steel_config: config::SteelConfig,
    ) -> Result<Self, SteelServerError> {
        Self::new_with_commands(
            chunk_runtime,
            cancel_token,
            steel_config,
            CommandRegistry::new(),
        )
        .await
    }

    /// Creates a new Steel server with additional commands registered atomically at startup.
    pub async fn new_with_commands(
        chunk_runtime: Arc<Runtime>,
        cancel_token: CancellationToken,
        steel_config: config::SteelConfig,
        command_registry: CommandRegistry,
    ) -> Result<Self, SteelServerError> {
        log::info!("Starting Steel Server ({GIT_HASH_SHORT})");

        let save_root = PathBuf::from(&steel_config.worlds.save_path);
        let session_lock = session_lock::SessionLock::acquire(&save_root).map_err(|source| {
            SteelServerError::SaveLocked {
                path: save_root.join(session_lock::LOCK_FILE),
                source,
            }
        })?;

        let permission_group_store = steel_config.permission_group_store();
        let server_port = steel_config.server.server_port;
        let worlds_config = steel_config.worlds;
        let permission_groups =
            PermissionGroupManager::new(steel_config.groups, permission_group_store).map_err(
                |error| {
                    SteelServerError::Core(format!("failed to validate groups config: {error}"))
                },
            )?;
        let player_idle_timeout = steel_config.server.player_idle_timeout;
        let runtime_config = steel_config.server.into_runtime_config();

        let server = Server::new_with_commands(
            chunk_runtime,
            cancel_token.clone(),
            runtime_config,
            worlds_config,
            permission_groups,
            command_registry,
        )
        .await
        .map_err(SteelServerError::Core)?;

        server
            .player_idle_timeout
            .store(player_idle_timeout, Ordering::Relaxed);

        let tcp_listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, server_port))
            .await
            .map_err(|source| SteelServerError::Bind {
                port: server_port,
                source,
            })?;

        Ok(Self {
            tcp_listener: Some(tcp_listener),
            cancel_token,
            client_id: 0,
            server: Arc::new(server),
            connection_session: Arc::new(ServerConnectionSession::default()),
            session_lock,
        })
    }

    /// Starts the server and begins accepting connections.
    pub async fn start(&mut self, task_tracker: TaskTracker) {
        log::info!("Started Steel Server");

        let server = Arc::clone(&self.server);
        let token = self.cancel_token.clone();
        let server_handle = tokio::spawn(async move {
            server.run(token).await;
        });

        loop {
            // Bind the accept future to the listener for this iteration only, so
            // the fields mutated below are not borrowed across it.
            let Some(listener) = self.tcp_listener.as_ref() else {
                break;
            };
            let accepted = select! {
                () = self.cancel_token.cancelled() => {
                    break;
                }
                accept_result = listener.accept() => accept_result,
            };
            let Ok((connection, address)) = accepted else {
                continue;
            };
            if let Err(e) = connection.set_nodelay(true) {
                log::warn!("Failed to set TCP_NODELAY: {e}");
            }
            let (java_client, sender_recv, net_reader) = JavaTcpClient::new(
                connection,
                address,
                self.client_id,
                self.cancel_token.child_token(),
                Arc::clone(&self.server),
                Arc::clone(&self.connection_session),
                task_tracker.clone(),
            );
            self.client_id = self.client_id.wrapping_add(1);
            log::info!(
                "Accepted connection from Java Edition: {address} (id {})",
                self.client_id
            );

            let java_client = Arc::new(java_client);
            java_client.start_outgoing_packet_task(sender_recv);
            java_client.start_incoming_packet_task(net_reader);
            // Java_client won't drop until the incoming and outcoming task close
            // So we dont need to care about them here anymore
        }
        // Vanilla logs `Stopping server` and closes the connection before it
        // saves; dropping the listener refuses new connections immediately.
        log::info!("Stopping server");
        self.tcp_listener = None;

        match server_handle.await {
            Ok(()) => {}
            Err(error) => {
                // Vanilla's server thread dying takes the process down; Steel
                // keeps going so the caller can still save and exit cleanly.
                log::error!("Server task ended unexpectedly: {error}");
            }
        }
    }
}
