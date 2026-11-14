//! A package without the data files built into the binary
//! (`simple_profile.json`, `sounds/`, `images/default/accent_colors.json`)
//! still boots and serves the same bytes as the full package.
//!
//! The binary built from this checkout is booted twice with
//! tests/boot/boot_harness.py in `--serve` mode, as the systemd service runs
//! it with the hardware faked: on the repository's data, then on a package
//! root holding only the default images. Both boots must log nothing at
//! ERROR, and the routes that serve the built-in data must answer the same.
//! `--serve` skips the Socket.IO checks, so only `python3` is needed.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

const ROUTES: [&str; 5] = [
    "/api/v1/profile/defaults",
    "/api/v1/profile/image",
    "/api/v1/sounds/list",
    "/api/v1/sounds/theme/list",
    "/api/v1/sounds/theme/get",
];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The status line, the content type and the body of `GET path`.
fn get(port: u16, path: &str) -> (String, Option<String>, Vec<u8>) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to the backend");
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
        .expect("read timeout");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .expect("send the request");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read the response");
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("end of the headers");
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap_or_default().to_string();
    let content_type = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.trim().to_string());
    (status, content_type, raw[split + 4..].to_vec())
}

struct Served {
    harness: Child,
    port: u16,
    report: PathBuf,
}

/// Boot the backend on `package_root` and wait until it serves.
fn serve(package_root: &Path, work: &Path) -> Served {
    let ready = work.join("ready.json");
    let report = work.join("report.json");
    let mut harness = Command::new("python3")
        .arg(repo_root().join("tests/boot/boot_harness.py"))
        .args(["--binary", env!("CARGO_BIN_EXE_meticulous-backend")])
        .arg("--package-root")
        .arg(package_root)
        .arg("--data-dir")
        .arg(work.join("data"))
        .arg("--report")
        .arg(&report)
        .arg("--serve")
        .arg(&ready)
        .stdout(Stdio::null())
        .spawn()
        .expect("run the boot harness");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let port = std::fs::read_to_string(&ready)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|ready| ready["port"].as_u64());
        if let Some(port) = port {
            let port = u16::try_from(port).expect("port number");
            return Served {
                harness,
                port,
                report,
            };
        }
        if let Some(status) = harness.try_wait().expect("harness status") {
            panic!(
                "the boot harness exited with {status} before the backend served: {}",
                std::fs::read_to_string(&report).unwrap_or_default()
            );
        }
        assert!(
            Instant::now() < deadline,
            "the backend did not serve within 60 s"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

impl Served {
    /// SIGTERM the harness, which stops the backend and writes its report;
    /// returns the report's failures (ERROR records included).
    fn stop(mut self) -> Value {
        let pid = nix::unistd::Pid::from_raw(i32::try_from(self.harness.id()).expect("pid"));
        nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGTERM).expect("stop the harness");
        self.harness.wait().expect("harness exit");
        let report: Value =
            serde_json::from_str(&std::fs::read_to_string(&self.report).expect("boot report"))
                .expect("boot report JSON");
        report["failures"].clone()
    }
}

fn copy_default_images(to: &Path) {
    let images = to.join("images/default");
    std::fs::create_dir_all(&images).expect("images folder");
    for entry in std::fs::read_dir(repo_root().join("images/default")).expect("repository images") {
        let path = entry.expect("entry").path();
        if path.extension().is_some_and(|e| e == "png") {
            std::fs::copy(&path, images.join(path.file_name().expect("name"))).expect("copy image");
        }
    }
}

#[test]
fn package_without_the_built_in_files_boots_and_serves_the_same_bytes() {
    if let Err(error) = Command::new("python3").arg("--version").output() {
        eprintln!("skipped: the boot harness needs python3 ({error})");
        return;
    }
    let tmp = tempfile::tempdir().expect("temporary directory");
    let stripped = tmp.path().join("package");
    copy_default_images(&stripped);
    std::fs::create_dir_all(stripped.join("api/web_ui")).expect("web_ui");

    let mut answers = Vec::new();
    for (name, package_root) in [("full", repo_root()), ("stripped", stripped)] {
        let work = tmp.path().join(name);
        std::fs::create_dir_all(&work).expect("work folder");
        let served = serve(&package_root, &work);
        let replies: Vec<_> = ROUTES.iter().map(|route| get(served.port, route)).collect();
        let failures = served.stop();
        assert_eq!(failures, Value::Array(vec![]), "{name} package boot");
        answers.push(replies);
    }
    for (route, (full, stripped)) in ROUTES.iter().zip(answers[0].iter().zip(&answers[1])) {
        assert!(full.0.contains(" 200 "), "{route}: {}", full.0);
        assert_eq!(stripped.0, full.0, "{route}");
        assert_eq!(stripped.1, full.1, "{route}");
        assert_eq!(
            String::from_utf8_lossy(&stripped.2),
            String::from_utf8_lossy(&full.2),
            "{route}"
        );
    }
}
