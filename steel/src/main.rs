//! Main entry point for the Steel Minecraft server.
#![feature(thread_id_value)]

use std::backtrace::{Backtrace, BacktraceStatus};
use std::panic::AssertUnwindSafe;
use std::path::{Path, absolute};
use std::sync::Arc;
use std::{env, fs, io, panic, process, thread};

use crossterm::style::Attribute::{Bold, Dim, Reset};
use crossterm::style::{Color, ResetColor, SetForegroundColor};
use futures::FutureExt;
use steel::config::{self, LogConfig};
use steel::logger::CommandLogger;
use steel::{SERVER, SteelServer, logger::LoggerLayer};
use steel_core::GIT_HASH_SHORT;
use steel_utils::text::DisplayResolutor;
#[cfg(all(windows, debug_assertions))]
use steel_utils::threading::DEBUG_STACK_SIZE;
use steel_utils::threading::{available_worker_threads, worker_threads_for_available};
use text_components::fmt::set_display_resolutor;
use tokio::runtime::{Builder, Runtime};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
#[cfg(feature = "jaeger")]
use tracing::Subscriber;
use tracing::{Level, error};
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};
#[cfg(feature = "jaeger")]
use tracing_subscriber::{Layer, registry::LookupSpan};

/// Command-line options, mirroring vanilla `Main`'s option set.
mod cli;
/// Mojang EULA acceptance check, mirroring vanilla `Eula`.
mod eula;
/// Cross-platform shutdown signal handling.
mod shutdown;

/// Path of the EULA acceptance file, resolved against the working directory
/// exactly like vanilla `Main#main`.
const EULA_PATH: &str = "eula.txt";

/// Path of the main configuration file, resolved against the working directory.
const CONFIG_PATH: &str = "config/config.toml";

#[cfg(feature = "jaeger")]
fn init_jaeger<S>() -> impl Layer<S> + Send + Sync
where
    S: Subscriber + for<'span> LookupSpan<'span> + Send + Sync,
{
    use opentelemetry::KeyValue;
    use opentelemetry::global;
    use opentelemetry::trace::TracerProvider;
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::trace::SdkTracerProvider;
    use tracing_opentelemetry::OpenTelemetryLayer;

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .build()
        .expect("Failed to create OTLP span exporter");

    let tracer_provider = SdkTracerProvider::builder()
        .with_resource(
            Resource::builder()
                .with_attributes([
                    KeyValue::new("service.name", "steel"),
                    KeyValue::new(
                        "service.build",
                        if cfg!(debug_assertions) {
                            "debug"
                        } else {
                            "release"
                        },
                    ),
                ])
                .build(),
        )
        .with_batch_exporter(exporter)
        .build();

    let tracer = tracer_provider.tracer("steel");
    global::set_tracer_provider(tracer_provider);
    OpenTelemetryLayer::new(tracer)
        .with_filter(EnvFilter::new("trace,h2=off,hyper=off,tonic=off,tower=off"))
}

async fn init_tracing(
    cancel_token: CancellationToken,
    log_config: Option<LogConfig>,
) -> Result<Arc<CommandLogger>, String> {
    let log_level = log_config
        .as_ref()
        .map_or(Level::INFO.into(), |l| l.log_level.to_directive());

    let tracing = tracing_subscriber::registry();

    #[cfg(feature = "jaeger")]
    let tracing = tracing.with(init_jaeger());

    let layer = LoggerLayer::new(cancel_token, log_config)
        .await
        .map_err(|err| format!("failed to initialize logger: {err}"))?;
    let logger = Arc::clone(&layer.0);

    let tracing = tracing.with(layer);

    let tracing = tracing.with(
        EnvFilter::builder()
            .with_default_directive(log_level)
            .from_env_lossy(),
    );

    set_display_resolutor(&DisplayResolutor);
    if let Err(err) = tracing.try_init() {
        logger.stop().await;
        return Err(format!("failed to initialize tracing subscriber: {err}"));
    }
    Ok(logger)
}

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

#[cfg(all(feature = "mimalloc", not(feature = "dhat-heap")))]
#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

// Windows defaults to a 1 MB main thread stack, which overflows in debug
// builds due to deeply nested generated density functions.
fn main() {
    #[cfg(all(windows, debug_assertions))]
    {
        thread::Builder::new()
            .name("steel-main".to_owned())
            .stack_size(DEBUG_STACK_SIZE)
            .spawn(steel_main)
            .expect("failed to spawn steel-main bootstrap thread")
            .join()
            .expect("steel-main thread panicked");
    }

    #[cfg(not(all(windows, debug_assertions)))]
    {
        steel_main();
    }
}

/// Why 2 runtimes?
///
/// The chunk runtime is very task heavy as it sometimes spawns thousands of tasks at once. It is also very await heavy in the part where it awaits its current layer.
///
/// If we only used one runtime this would lead to the tick task being blocked by the chunk tasks.
///
/// We have to create the runtimes at this level cause tokio panics if you drop a runtime in a context where blocking is not allowed.
#[expect(
    clippy::unwrap_used,
    reason = "runtime build failures are fatal and unrecoverable at startup"
)]
fn steel_main() {
    #[cfg(feature = "dhat-heap")]
    let _profiler = dhat::Profiler::new_heap();

    // Vanilla parses its options before touching any settings, prints help on
    // `--help`, and returns without booting when parsing fails.
    let args = match cli::CliArgs::parse(env::args_os()) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return;
        }
    };
    if let Some(path) = &args.pid_file {
        if let Err(error) = write_pid_file(path) {
            eprintln!("Failed to write pid file {}: {error}", path.display());
            return;
        }
    }

    // Load config once at startup
    let mut steel_config = match config::load_or_create(Path::new(CONFIG_PATH)) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("Failed to load configuration: {error}");
            return;
        }
    };
    if let Err(error) = apply_startup_overrides(&mut steel_config, &args) {
        eprintln!("{error}");
        return;
    }

    // Vanilla constructs its `Eula` before the `--initSettings` check, so a
    // fresh run always leaves an `eula.txt` behind, then refuses to boot until
    // the operator agrees.
    let eula = eula::Eula::load(Path::new(EULA_PATH));
    if args.init_settings {
        println!(
            "Initialized '{}' and '{}'",
            display_path(Path::new(CONFIG_PATH)),
            display_path(Path::new(EULA_PATH))
        );
        return;
    }

    let main_worker_threads = worker_threads_for_available(
        steel_config.server.threads.main_runtime,
        available_worker_threads(),
    );
    let chunk_worker_threads = worker_threads_for_available(
        steel_config.server.threads.chunk_runtime,
        available_worker_threads(),
    );

    let chunk_runtime = Arc::new(
        Builder::new_multi_thread()
            .worker_threads(chunk_worker_threads)
            .thread_name("chunk-worker")
            .enable_all()
            .build()
            .unwrap(),
    );

    let main_runtime = Builder::new_multi_thread()
        .worker_threads(main_worker_threads)
        .thread_name("main-worker")
        .enable_all()
        .build()
        .unwrap();

    main_runtime.block_on(main_async(Arc::clone(&chunk_runtime), steel_config, eula));

    drop(main_runtime);
    drop(chunk_runtime);
}

async fn main_async(
    chunk_runtime: Arc<Runtime>,
    steel_config: config::SteelConfig,
    eula: eula::Eula,
) {
    let cancel_token = CancellationToken::new();

    let logger = match init_tracing(cancel_token.clone(), steel_config.log.clone()).await {
        Ok(logger) => logger,
        Err(error) => {
            eprintln!("{error}");
            return;
        }
    };
    if eula.wrote_defaults() {
        log::warn!("Failed to load {EULA_PATH}; wrote fresh defaults");
    }
    if !eula.has_agreed_to_eula() {
        // Vanilla `Main#main` message, logged at INFO level.
        log::info!(
            "You need to agree to the EULA in order to run the server. Go to {EULA_PATH} for more info."
        );
        logger.stop().await;
        return;
    }
    shutdown::install(cancel_token.clone());
    let panic_token = cancel_token.clone();
    panic::set_hook(Box::new(move |panic_info| {
        let message = panic_info.payload_as_str().unwrap_or("Unknown");
        // Also write the report synchronously to stderr before the logger's channel is
        // used: `error!` queues onto the async `CommandLogger`, so a thread that aborts
        // instead of unwinding (rayon's abort-on-panic guard kills the process) loses
        // every queued line and the crash has no recorded cause at all.
        let thread = thread::current();
        let thread_name = thread.name().unwrap_or("unnamed");
        let location = panic_info.location().map_or_else(
            || "an unknown location".to_owned(),
            |location| {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            },
        );
        eprintln!("[FATAL ERROR] Thread '{thread_name}' has panicked at {location}: {message}");
        let backtrace = Backtrace::capture();
        if backtrace.status() == BacktraceStatus::Captured {
            eprintln!("Stack Backtrace:\n{backtrace}");
        }
        error!(
            "{}Steel {} ({GIT_HASH_SHORT}) {} on {}/{}{}",
            SetForegroundColor(Color::Red),
            env!("CARGO_PKG_VERSION"),
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            env::consts::OS,
            env::consts::ARCH,
            ResetColor
        );
        let current_thread = thread::current();
        let thread_name = current_thread.name().unwrap_or("unnamed");
        let thread_id = current_thread.id();
        if let Some(location) = panic_info.location() {
            error!(
                "{}Thread '{thread_name}' ({}) has panicked at {}:{}:{}{}",
                SetForegroundColor(Color::Red),
                thread_id.as_u64(),
                location.file(),
                location.line(),
                location.column(),
                ResetColor
            );
        } else {
            error!(
                "{}Thread '{thread_name}' ({}) has panicked at an unknown location{}",
                SetForegroundColor(Color::Red),
                thread_id.as_u64(),
                ResetColor
            );
        }
        error!(
            "{}{}[FATAL ERROR]{}{} {message}{}",
            SetForegroundColor(Color::Red),
            Bold,
            Reset,
            SetForegroundColor(Color::Red),
            ResetColor
        );

        match backtrace.status() {
            BacktraceStatus::Captured => {
                error!("Stack Backtrace:");
                for trace in backtrace.to_string().split('\n') {
                    error!("{}", trace.trim_start());
                }
            }
            BacktraceStatus::Disabled => {
                error!(
                    "{}Backtrace is disabled. Run with RUST_BACKTRACE=1 to enable it.{}",
                    Dim, Reset
                );
            }
            BacktraceStatus::Unsupported => {
                error!(
                    "{}Backtrace capability is not supported on this platform.{}",
                    Dim, Reset
                );
            }
            _ => {}
        }

        panic_token.cancel();
    }));

    let run_result = AssertUnwindSafe(run_server(chunk_runtime, cancel_token, steel_config))
        .catch_unwind()
        .await;
    let panic_payload = match run_result {
        Ok(Ok(())) => None,
        Ok(Err(error)) => {
            log::error!("Server startup failed: {error}");
            None
        }
        Err(payload) => Some(payload),
    };

    logger.stop().await;

    if let Some(payload) = panic_payload {
        panic::resume_unwind(payload);
    }
}

async fn run_server(
    chunk_runtime: Arc<Runtime>,
    cancel_token: CancellationToken,
    steel_config: config::SteelConfig,
) -> Result<(), String> {
    #[cfg(feature = "deadlock_detection")]
    {
        // only for #[cfg]
        use parking_lot::deadlock;
        use std::thread;
        use std::time::Duration;

        // Create a background thread which checks for deadlocks every 10s
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_secs(10));
                let deadlocks = deadlock::check_deadlock();
                if deadlocks.is_empty() {
                    continue;
                }

                log::error!("{} deadlocks detected", deadlocks.len());
                for (i, threads) in deadlocks.iter().enumerate() {
                    log::error!("Deadlock #{i}");
                    for t in threads {
                        log::error!("Thread Id {:#?}", t.thread_id());
                        log::error!("{:#?}", t.backtrace());
                    }
                }
            }
        });
    }

    let mut steel = SteelServer::new(
        Arc::clone(&chunk_runtime),
        cancel_token.clone(),
        steel_config,
    )
    .await
    .map_err(|e| e.to_string())?;

    let server = Arc::clone(&steel.server);
    let task_tracker = TaskTracker::new();

    // Vanilla runs `stopServer` from a `finally`, so an unexpected panic still
    // saves player and world data before the process exits.
    let run_result = AssertUnwindSafe(run_started_server(&mut steel, &task_tracker))
        .catch_unwind()
        .await;

    // Either way, stop accepting work and drain the tracker before saving:
    // `save_and_shutdown` requires packet processing to have stopped.
    log::info!("Waiting for pending tasks...");
    task_tracker.close();
    task_tracker.wait().await;

    server.save_and_shutdown().await;

    match run_result {
        Ok(()) => {
            log::info!("Server stopped");
            Ok(())
        }
        Err(payload) => {
            log::error!("Server thread panicked; saved world and player data before shutting down");
            panic::resume_unwind(payload);
        }
    }
}

/// Runs the accept loop until shutdown.
///
/// Split out of [`run_server`] so the caller can drain spawned tasks and save
/// data even when this unwinds.
async fn run_started_server(steel: &mut SteelServer, task_tracker: &TaskTracker) {
    let server = Arc::clone(&steel.server);

    if !server.prepare_spawn_area().await {
        return;
    }

    SERVER.set(Arc::clone(&steel.server)).ok();

    steel.start(task_tracker.clone()).await;
}

/// Applies the command-line overrides vanilla's launcher exposes.
///
/// `--port` overrides `server.server_port` unless it is negative, matching
/// vanilla's `-1` default that falls back to `server.properties`. `--universe`
/// replaces the save root and `--world` selects the default domain, Steel's
/// equivalents of vanilla's universe directory and `level-name`.
fn apply_startup_overrides(
    steel_config: &mut config::SteelConfig,
    args: &cli::CliArgs,
) -> Result<(), String> {
    if let Some(port) = args.port {
        if port < 0 {
            // Vanilla keeps `server-port` when `--port` is negative.
        } else {
            steel_config.server.server_port = u16::try_from(port)
                .map_err(|_| format!("--port must fit in 0..=65535, got {port}"))?;
        }
    }
    if let Some(universe) = &args.universe {
        // Vanilla resolves `--universe` against the working directory. Steel
        // rejects `.`/`..` components in `save_path`, so normalize first.
        let resolved = absolute(universe).unwrap_or_else(|_| universe.clone());
        steel_config.worlds.save_path = resolved.to_string_lossy().into_owned();
    }
    if let Some(world) = &args.world {
        steel_config.select_default_domain(world)?;
    }
    Ok(())
}

/// Writes the process id to `path`, like vanilla `Main#writePidFile`.
fn write_pid_file(path: &Path) -> io::Result<()> {
    fs::write(path, process::id().to_string())
}

/// Absolute display path for startup messages (vanilla `Path#toAbsolutePath`).
fn display_path(path: &Path) -> String {
    absolute(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}
