//! `meticulous-backend`: the espresso machine's backend service.
//!
//! With no arguments it serves (what `/usr/bin/meticulous-backend` started in
//! the Python package). `met-config` reads and edits the settings file.

// The port lands piece by piece: services expose what the pieces still being
// ported (the machine runtime, the settings API) will call.
#![allow(dead_code)]

mod alarms;
mod api;
mod app;
mod backlight;
mod clock;
mod config;
mod contract;
mod dbus_monitor;
mod debug_shots;
mod device_identity;
mod env;
mod esp_tool;
mod events;
mod host;
mod http;
mod imager;
mod images;
mod limited_access;
mod logging;
mod machine;
mod machine_info;
mod manual_mode;
mod metconfig;
mod notifications;
mod os_update;
mod ota;
mod pending;
mod pour_over;
mod profiles;
mod redaction;
mod sentry_reporting;
mod shot_database;
mod server;
mod shots;
mod simple_profile;
mod sio;
mod smoke_validation;
mod sounds;
mod ssh_manager;
mod system_services;
#[cfg(test)]
mod testing;
mod usb;

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
    /// Print the HTTP routes, Socket.IO events, payload keys and default
    /// settings as the contract snapshots record them.
    Contract,
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
        Command::Contract => contract(),
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
    // back.py initializes Sentry at import, before anything else runs. A run
    // with the hardware faked never reports.
    let reporting = sentry_reporting::Reporting::init(env.sentry && !env.hooks.fake_hardware);
    runtime.block_on(async move {
        let app = app::App::new(env, log, reporting);
        // back.run(): the history database opens and migrates before main().
        let history_app = app.clone();
        let _ = tokio::task::spawn_blocking(move || history_app.history.init()).await;
        if let Err(error) = start(&app).await {
            tracing::error!("main() failed: {error}");
            return std::process::ExitCode::from(1);
        }
        let router = Arc::new(api::router(&app));
        tokio::spawn(events::live(app.clone()));
        app.imager.flash_if_required();
        match server::serve(app, router).await {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!("main() failed: {error}");
                std::process::ExitCode::from(1)
            }
        }
    })
}

/// `meticulous-backend contract`: built from a throwaway environment so it
/// touches nothing of the machine's.
fn contract() -> std::process::ExitCode {
    let Ok(dir) = tempfile::tempdir() else {
        return std::process::ExitCode::from(1);
    };
    let package_root = env::Env::from_process().paths.package_root;
    let env = env::Env::isolated(dir.path(), package_root, env::BackendMode::Emulator);
    let log = std::sync::Arc::new(logging::LogHooks::default());
    let app = app::App::new(env, log, sentry_reporting::Reporting::disabled());
    contract::print(&app);
    std::process::ExitCode::SUCCESS
}

/// `backend.main()` before the server listens, in the Python's order.
async fn start(app: &Arc<app::App>) -> Result<(), String> {
    redaction::install(app);
    app.dbus.init().await;
    // HostnameManager.init
    app.ota.init();
    app.alarms.init();
    app.machine.init(app)?;
    app.ssh
        .init(app.machine.enable_manufacturing(), app.emulated());
    app.system_services.init(app.emulated()).await;
    app.usb.init(app.emulated());
    app.dbus.enable_usb_test().await;
    events::start_stdin_commands(app.clone());
    app.profiles.init(app);
    app.pour_over.init();
    app.sounds.init(app.emulated(), true);
    Ok(())
}
