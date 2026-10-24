//! `meticulous-backend`: the espresso machine's backend service.
//!
//! With no arguments it serves (what `/usr/bin/meticulous-backend` started in
//! the Python package). `met-config` reads and edits the settings file.

mod alarms;
mod api;
mod app;
mod ble;
mod config;
mod env;
mod events;
mod hostname;
mod http;
mod http_client;
mod images;
mod logging;
mod notifications;
mod py;
mod qr;
mod server;
mod sio;
mod sounds;
#[cfg(test)]
mod testing;
mod timezone;
mod wifi;
mod zeroconf;

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
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => serve(),
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
        let listening = app.clone();
        match server::serve(app, router, move || after_listen(&listening)).await {
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
    app.hostname.init();
    app.alarms.init();
    // `GATTServer.getServer().start()`; the boot harness patches out BlueZ.
    if !app.env.hooks.fake_hardware {
        app.gatt.start(&app.wifi);
    }
    if app.emulated() {
        init_wifi_emulated(app);
    }
    app.sounds.init(app.emulated(), true);
    app.timezone.init();
}

/// `WifiManager.init()`, run inline on the emulator. With fake hardware only
/// the NetworkManager probe runs, so the emulation routes still replace the
/// real ones when `nmcli` is missing, and nothing changes the host.
fn init_wifi_emulated(app: &Arc<app::App>) {
    if app.env.hooks.fake_hardware {
        app.wifi.probe_networking();
        return;
    }
    if let Err(error) = app.wifi.init() {
        tracing::error!("WifiManager.init failed: {}: {}", error.kind, error.message);
    }
}

/// `backend.main()` once the server listens.
fn after_listen(app: &Arc<app::App>) {
    // `start_wifi_manager_in_background()`, which the boot harness patches out.
    if !app.emulated() && !app.env.hooks.fake_hardware {
        app.wifi.start_in_background();
    }
}
