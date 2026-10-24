//! `meticulous-backend`: the espresso machine's backend service.
//!
//! With no arguments it serves (what `/usr/bin/meticulous-backend` started in
//! the Python package). `met-config` reads and edits the settings file.

mod alarms;
mod api;
mod app;
mod config;
mod env;
mod events;
mod http;
mod images;
mod logging;
mod metconfig;
mod notifications;
mod server;
mod sio;
mod sounds;
#[cfg(test)]
mod testing;

use std::sync::Arc;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "meticulous-backend",
    version,
    about = "Meticulous espresso machine backend"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the HTTP and Socket.IO API (the default).
    Serve,
    /// Inspect and modify the configuration (met-config.py).
    MetConfig(metconfig::Args),
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(),
        Command::MetConfig(args) => {
            let level = std::env::var("LOGLEVEL").unwrap_or_else(|_| "DEBUG".into());
            logging::init(&level.to_uppercase());
            metconfig::run(args)
        }
    }
}

fn serve() -> std::process::ExitCode {
    let env = env::Env::from_process();
    let log = logging::init(&env.log_level);
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(64)
        .thread_name("tokio")
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!("main() failed: cannot start the runtime: {error}");
            return std::process::ExitCode::from(1);
        }
    };
    runtime.block_on(async move {
        let app = app::App::new(env, log);
        start(&app);
        let router = Arc::new(api::router(&app));
        match server::serve(app, router).await {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!("main() failed: {error}");
                std::process::ExitCode::from(1)
            }
        }
    })
}

/// `backend.main()` before the server listens, in the Python's order.
fn start(app: &Arc<app::App>) {
    app.alarms.init();
    app.sounds.init(app.emulated(), true);
}
