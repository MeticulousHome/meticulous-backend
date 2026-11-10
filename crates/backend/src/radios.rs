//! The Wi-Fi and Bluetooth radios. `radios_enabled` (the Dial's Wi-Fi menu)
//! turns both off and keeps them off across reboots; plain standby turns them
//! off only while the machine sleeps, and hands them back as that setting has
//! them.

use std::sync::Arc;
use std::time::Duration;

use crate::app::App;
use crate::config::{ConfigStore, keys};

const NMCLI_TIMEOUT: Duration = Duration::from_secs(10);

/// Whether the user wants the radios on; on unless they turned them off.
pub fn enabled(config: &ConfigStore) -> bool {
    config
        .get(keys::USER, keys::RADIOS_ENABLED)
        .as_bool()
        .unwrap_or(true)
}

/// Switch the Wi-Fi radio through NetworkManager. The auto-connect loop
/// leaves a radio that is off alone.
pub fn set_wifi(app: &App, on: bool) {
    app.wifi.set_radio_off(!on);
    let argv = ["nmcli", "radio", "wifi", if on { "on" } else { "off" }];
    match app.host.run_with_timeout(&argv, NMCLI_TIMEOUT) {
        Ok(output) if output.success() => {
            tracing::info!("Wi-Fi radio {}", if on { "on" } else { "off" });
        }
        Ok(output) => tracing::warn!(
            "Could not switch the Wi-Fi radio: {}",
            crate::host::describe_failure(&argv, &output)
        ),
        Err(error) => tracing::warn!("Could not switch the Wi-Fi radio: {error}"),
    }
}

/// Switch both radios.
pub async fn apply(app: &Arc<App>, on: bool) {
    let worker = app.clone();
    if let Err(error) = tokio::task::spawn_blocking(move || set_wifi(&worker, on)).await {
        tracing::error!("Switching the Wi-Fi radio panicked: {error}");
    }
    app.gatt.set_radio_powered(on).await;
}

/// At boot the setting wins over whatever state the radios were left in.
/// Bluetooth comes up later, with the GATT server, which checks it too.
pub fn start(app: Arc<App>) {
    if !crate::standby::available(&app) {
        return;
    }
    let on = enabled(&app.config);
    tokio::spawn(async move { apply(&app, on).await });
}

#[cfg(test)]
mod tests {
    //! A machine the user took off the air must stay off it, and one they
    //! put back must come back: both radios follow the one setting.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::host::fake::FakeRunner;
    use crate::testing;
    use serde_json::json;

    #[test]
    fn the_radios_are_on_unless_the_user_turned_them_off() {
        let (_dir, app) = testing::app();
        assert!(enabled(&app.config));
        app.config
            .set_and_save(keys::USER, keys::RADIOS_ENABLED, json!(false));
        assert!(!enabled(&app.config));
    }

    #[test]
    fn the_wifi_radio_is_switched_through_networkmanager_and_auto_connect_follows() {
        let (_dir, app) = testing::app();
        let runner = Arc::new(FakeRunner::default());
        app.host.replace(runner.clone());

        set_wifi(&app, false);
        assert!(app.wifi.radio_off());
        set_wifi(&app, true);
        assert!(!app.wifi.radio_off());

        assert_eq!(
            runner.argvs(),
            [
                ["nmcli", "radio", "wifi", "off"],
                ["nmcli", "radio", "wifi", "on"]
            ]
        );
    }
}
