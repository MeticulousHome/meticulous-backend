//! tests/test_boot.py: boot the backend built from this checkout with
//! tests/boot/boot_harness.py, fresh and then again on the same data
//! directory, where it finds the config, profiles and history of the first
//! boot.
//!
//! The harness is a Socket.IO client and needs python-socketio and aiohttp
//! next to `python3`. Like the Python test, which needs the machine
//! dependency group, this one is skipped where they are missing, CI's
//! `cargo test` included: CI boots the binary with the same harness in the
//! Package workflow, from the installed .deb ("Installed package boots
//! twice").

use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Why the harness cannot run here, if it cannot.
fn harness_unavailable() -> Option<String> {
    match Command::new("python3")
        .args(["-c", "import socketio, aiohttp"])
        .output()
    {
        Err(error) => Some(format!("python3: {error}")),
        Ok(output) if !output.status.success() => Some(
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .last()
                .unwrap_or_default()
                .to_string(),
        ),
        Ok(_) => None,
    }
}

/// tests/test_boot.py::test_backend_boots_fresh_and_again_with_its_own_data
#[test]
fn backend_boots_fresh_and_again_with_its_own_data() {
    if let Some(reason) = harness_unavailable() {
        eprintln!("skipped: the boot harness needs python-socketio and aiohttp ({reason})");
        return;
    }
    let harness = repo_root().join("tests/boot/boot_harness.py");
    let tmp = tempfile::tempdir().expect("temporary directory");
    let data_dir = tmp.path().join("data");
    for boot in 1..=2 {
        let report_path = tmp.path().join(format!("boot-{boot}.json"));
        // The harness picks a free port itself and gives up after its own
        // watchdog (BOOT_TIMEOUT_SECONDS).
        let result = Command::new("python3")
            .arg(&harness)
            .args(["--binary", env!("CARGO_BIN_EXE_meticulous-backend")])
            .arg("--data-dir")
            .arg(&data_dir)
            .arg("--report")
            .arg(&report_path)
            .args(["--boot-index", &boot.to_string()])
            .output()
            .expect("run the boot harness");
        let failures = std::fs::read_to_string(&report_path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .map(|report| report["failures"].clone())
            .unwrap_or_default();
        let stderr = String::from_utf8_lossy(&result.stderr);
        let tail_start = stderr
            .char_indices()
            .rev()
            .nth(4000)
            .map_or(0, |(index, _)| index);
        assert!(
            result.status.success(),
            "boot {boot} failed: {failures}\n{}",
            &stderr[tail_start..]
        );
    }
}
