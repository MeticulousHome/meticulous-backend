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
mod ble;
mod bug_report;
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
mod hostname;
mod http;
mod http_client;
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
#[cfg(test)]
mod ported;
mod pour_over;
mod profiles;
mod py;
mod qr;
mod redaction;
mod sentry_reporting;
mod server;
mod shot_data_sharing;
mod shot_database;
mod shots;
mod simple_profile;
mod sio;
mod smoke_validation;
mod sounds;
mod ssh_manager;
mod system_services;
#[cfg(test)]
mod testing;
mod timezone;
mod usb;
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
    blocking(app, |app| app.hostname.init()).await;
    app.ota.init();
    app.alarms.init();
    app.machine.init(app)?;
    let machine = Arc::downgrade(app);
    app.wifi.set_manufacturing_probe(Arc::new(move || {
        machine
            .upgrade()
            .is_some_and(|app| app.machine.enable_manufacturing())
    }));
    app.ssh
        .init(app.machine.enable_manufacturing(), app.emulated());
    app.system_services.init(app.emulated()).await;
    app.usb.init(app.emulated());
    app.dbus.enable_usb_test().await;
    events::start_stdin_commands(app.clone());
    // `GATTServer.getServer().start()`; the boot harness patches out BlueZ.
    if !app.env.hooks.fake_hardware {
        app.gatt.start(&app.wifi);
    }
    if app.emulated() {
        blocking(app, init_wifi_emulated).await;
    }
    app.profiles.init(app);
    app.pour_over.init();
    app.sounds.init(app.emulated(), true);
    blocking(app, |app| app.timezone.init()).await;
    blocking(app, bug_report::start).await;
    Ok(())
}

/// Run a step that blocks (subprocesses, blocking HTTP) off the runtime.
async fn blocking(app: &Arc<app::App>, step: impl FnOnce(&Arc<app::App>) + Send + 'static) {
    let app = app.clone();
    if let Err(error) = tokio::task::spawn_blocking(move || step(&app)).await {
        tracing::error!("main() step panicked: {error}");
    }
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
    tokio::spawn(events::live(app.clone()));
    app.imager.flash_if_required();
}
