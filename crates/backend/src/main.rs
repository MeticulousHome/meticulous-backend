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
mod dbus_monitor;
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
mod notifications;
mod os_update;
mod ota;
mod server;
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
        start(&app).await;
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
async fn start(app: &Arc<app::App>) {
    app.dbus.init().await;
    // HostnameManager.init
    app.ota.init();
    app.alarms.init();
    // Machine.init
    app.ssh.init(manufacturing_mode(app), app.emulated());
    app.system_services.init(app.emulated()).await;
    app.usb.init(app.emulated());
    app.dbus.enable_usb_test().await;
    app.sounds.init(app.emulated(), true);
    app.imager.flash_if_required();
}

/// `Machine.enable_manufacturing` as `Machine.init` leaves it: forced by the
/// settings, or no real serial yet (Machine.init gives a machine without one
/// a random "999..." serial). Until the machine runtime is wired in, the same
/// decision is read off the settings here.
fn manufacturing_mode(app: &app::App) -> bool {
    use config::keys;
    if app
        .config
        .get_bool(keys::MANUFACTURING, keys::FORCE_MANUFACTURING_ENABLED)
    {
        return true;
    }
    let serial = match app.config.get(keys::SYSTEM, keys::MACHINE_SERIAL_NUMBER) {
        serde_json::Value::String(serial) => serial,
        serde_json::Value::Number(number) => number.to_string(),
        _ => String::new(),
    };
    serial.is_empty() || serial == "NOT_ASSIGNED" || serial.starts_with("999")
}
