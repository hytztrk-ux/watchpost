use std::io::{IsTerminal, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::EnvFilter;

use watchpost::config::{Config, EXAMPLE_CONFIG};
use watchpost::monitor::{CheckResult, Status};
use watchpost::notify::{Dispatcher, Notification};
use watchpost::scheduler::{self, Scheduler};
use watchpost::store::Store;
use watchpost::web;

/// Single-binary uptime and service monitor with a built-in status page.
#[derive(Parser)]
#[command(name = "watchpost", version, about, long_about = None)]
struct Cli {
    /// Path of the configuration file.
    #[arg(
        short,
        long,
        global = true,
        env = "WATCHPOST_CONFIG",
        default_value = "watchpost.toml"
    )]
    config: PathBuf,

    /// Log verbosity. Defaults to `info` for `run` and `warn` otherwise;
    /// RUST_LOG is used when this flag is not given.
    #[arg(long, global = true, value_enum)]
    log_level: Option<LogLevel>,

    /// Log output format.
    #[arg(long, global = true, value_enum, default_value_t = LogFormat::Text)]
    log_format: LogFormat,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Write a commented example configuration file.
    Init {
        /// Overwrite an existing file.
        #[arg(long)]
        force: bool,
    },
    /// Validate the configuration and run every monitor once.
    Check {
        /// Only validate the configuration, do not run any checks.
        #[arg(long)]
        config_only: bool,
    },
    /// Run the monitors and the status page until stopped.
    Run,
    /// Send a test message through the configured notifiers.
    TestNotify {
        /// Only this notifier (default: all).
        name: Option<String>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    fn as_str(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum LogFormat {
    Text,
    Json,
}

fn init_logging(cli: &Cli) {
    let default_level = match cli.command {
        Command::Run => LogLevel::Info,
        _ => LogLevel::Warn,
    };
    let filter = match cli.log_level {
        Some(level) => EnvFilter::new(format!("warn,watchpost={}", level.as_str())),
        None => EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new(format!("warn,watchpost={}", default_level.as_str()))
        }),
    };
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false);
    if cli.log_format == LogFormat::Json {
        builder.json().flatten_event(true).init();
    } else {
        builder
            .with_ansi(std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none())
            .init();
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging(&cli);

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(4)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: cannot start runtime: {e}");
            return ExitCode::FAILURE;
        }
    };

    let result = runtime.block_on(async {
        match &cli.command {
            Command::Init { force } => cmd_init(&cli.config, *force),
            Command::Check { config_only } => cmd_check(&cli.config, *config_only).await,
            Command::Run => cmd_run(&cli.config).await,
            Command::TestNotify { name } => cmd_test_notify(&cli.config, name.as_deref()).await,
        }
    });
    // Do not wait for lingering blocking tasks (e.g. slow DNS lookups).
    runtime.shutdown_timeout(Duration::from_secs(1));

    match result {
        Ok(code) => code,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::from(2)
        }
    }
}

type CmdResult = Result<ExitCode, String>;

fn load(path: &Path) -> Result<Config, String> {
    Config::load(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn cmd_init(path: &Path, force: bool) -> CmdResult {
    if path.exists() && !force {
        return Err(format!(
            "{} already exists (use --force to overwrite)",
            path.display()
        ));
    }
    std::fs::write(path, EXAMPLE_CONFIG)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    println!("Wrote {}", path.display());
    println!("Edit it, then run `watchpost check` and `watchpost run`.");
    Ok(ExitCode::SUCCESS)
}

async fn cmd_check(path: &Path, config_only: bool) -> CmdResult {
    let cfg = load(path)?;
    println!(
        "Configuration OK: {} monitor(s), {} notifier(s).",
        cfg.monitors.len(),
        cfg.notifiers.len()
    );
    if config_only {
        return Ok(ExitCode::SUCCESS);
    }
    println!();
    let results = scheduler::check_all(&cfg)
        .await
        .map_err(|e| e.to_string())?;
    print_table(&cfg, &results);
    let down = results.iter().filter(|r| r.status == Status::Down).count();
    println!();
    if down == 0 {
        println!("All {} monitor(s) are up.", results.len());
        Ok(ExitCode::SUCCESS)
    } else {
        println!("{down} of {} monitor(s) are down.", results.len());
        Ok(ExitCode::from(1))
    }
}

fn print_table(cfg: &Config, results: &[CheckResult]) {
    let color = std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    let rows: Vec<[String; 5]> = cfg
        .monitors
        .iter()
        .zip(results)
        .map(|(m, r)| {
            [
                m.name.clone(),
                m.kind.type_name().to_owned(),
                r.status.as_str().to_uppercase(),
                r.latency
                    .map(|d| format!("{} ms", d.as_millis()))
                    .unwrap_or_else(|| "-".into()),
                r.message.clone(),
            ]
        })
        .collect();
    let headers = ["NAME", "TYPE", "STATUS", "LATENCY", "DETAIL"];
    let mut widths = headers.map(|h| h.chars().count());
    for row in &rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    let mut out = std::io::stdout().lock();
    let line = |cells: &[String; 5], status: Option<Status>| -> String {
        let mut s = String::new();
        for (i, cell) in cells.iter().enumerate() {
            let pad = widths[i].saturating_sub(cell.chars().count());
            let text = match (i, status, color) {
                (2, Some(st), true) => {
                    let code = match st {
                        Status::Up => "32",
                        Status::Warn => "33",
                        Status::Down => "31",
                    };
                    format!("\x1b[1;{code}m{cell}\x1b[0m")
                }
                _ => cell.clone(),
            };
            s.push_str(&text);
            if i + 1 < cells.len() {
                s.push_str(&" ".repeat(pad + 2));
            }
        }
        s
    };
    let _ = writeln!(out, "{}", line(&headers.map(String::from), None));
    for (row, r) in rows.iter().zip(results) {
        let _ = writeln!(out, "{}", line(row, Some(r.status)));
    }
}

async fn cmd_test_notify(path: &Path, only: Option<&str>) -> CmdResult {
    let cfg = load(path)?;
    if cfg.notifiers.is_empty() {
        return Err("no notifiers configured (add a [[notifier]] section)".into());
    }
    let dispatcher = Dispatcher::new(&cfg).map_err(|e| e.to_string())?;
    let names: Vec<String> = match only {
        Some(n) if dispatcher.names().iter().any(|x| x == n) => vec![n.to_owned()],
        Some(n) => return Err(format!("unknown notifier `{n}`")),
        None => dispatcher.names(),
    };
    let msg = Notification::test(scheduler::unix_now());
    let mut failed = 0;
    for name in &names {
        match dispatcher.send_to(name, &msg).await {
            Ok(()) => println!("{name}: sent"),
            Err(e) => {
                failed += 1;
                println!("{name}: FAILED: {e}");
            }
        }
    }
    Ok(if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

async fn cmd_run(path: &Path) -> CmdResult {
    let cfg = Arc::new(load(path)?);
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        config = %path.display(),
        monitors = cfg.monitors.len(),
        notifiers = cfg.notifiers.len(),
        "starting watchpost"
    );

    let store = Store::open(&cfg.storage.path)
        .map_err(|e| format!("{}: {e}", cfg.storage.path.display()))?;
    tracing::info!(path = %cfg.storage.path.display(), "database opened");
    let dispatcher = Dispatcher::new(&cfg).map_err(|e| e.to_string())?;
    let live = scheduler::new_live_state(&cfg);
    let shutdown = CancellationToken::new();

    let web_task = if cfg.web.enabled {
        let addr = cfg.web.listen_addr();
        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| format!("cannot listen on {addr}: {e}"))?;
        tracing::info!(
            url = %format!("http://{}", listener.local_addr().unwrap_or(addr)),
            auth = cfg.web.auth.is_some(),
            "status page listening"
        );
        let state = web::AppState {
            cfg: cfg.clone(),
            store: store.clone(),
            live: live.clone(),
        };
        let token = shutdown.clone();
        Some(tokio::spawn(async move {
            if let Err(e) = web::serve(listener, state, token.clone()).await {
                tracing::error!(error = %e, "web server failed");
                token.cancel();
            }
        }))
    } else {
        None
    };

    let token = shutdown.clone();
    tokio::spawn(async move {
        wait_for_signal().await;
        tracing::info!("shutting down (press Ctrl-C again to force)");
        token.cancel();
        wait_for_signal().await;
        tracing::warn!("forced exit");
        std::process::exit(130);
    });

    let scheduler = Scheduler::new(cfg.clone(), store.clone(), dispatcher.clone(), live);
    let result = scheduler.run(shutdown.clone()).await;
    shutdown.cancel();
    if let Some(task) = web_task {
        let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
    }
    dispatcher.shutdown(Duration::from_secs(10)).await;
    if let Err(e) = store.run(|s| s.checkpoint()).await {
        tracing::warn!(error = %e, "cannot checkpoint database");
    }
    result.map_err(|e| e.to_string())?;
    tracing::info!("stopped");
    Ok(ExitCode::SUCCESS)
}

async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
