# The Rust backend

This repository is moving from Python to Rust. The Rust backend is a drop-in
replacement: the same `.deb` name, the same `/usr/bin/meticulous-backend`, the
same HTTP routes, Socket.IO events, bytes on the ESP32 UART and files under
`/meticulous-user`. The Dial, the mobile app, meticulous-machine and the
firmware do not change.

The Python sources stay in the tree until the Rust backend has passed every
gate and run on a machine; they are the reference. Where this document says
"the Python", it means the code on `beta` at the commit this branch started
from.

## Layout

```
Cargo.toml                 workspace
crates/pyfmt/              CPython-compatible float repr, json.dumps, float()/int()/unquote
crates/esp/                ESP32 UART protocol, ESP observability, motor energy, firmware versions (pure)
crates/profile/            profile variables, the manual-mode node program, schema validation (pure)
crates/redactor/           log redaction, sentry event scrubbing, reportable config (pure)
crates/history/            the SQLite history store, its alembic-compatible migrations, shot and Pour Over files (sync IO)
crates/backend/            the `meticulous-backend` binary: HTTP, Socket.IO, the machine, storage, OS
tests/                     black-box tests that drive the binary over HTTP, Socket.IO and a pty
```

Pure crates have no IO, no globals, no async and no clocks: time comes in as
an argument. Everything that touches the network, the disk, D-Bus, GPIO or the
UART lives in `crates/backend`, except `crates/history`: synchronous disk IO
with its paths passed in, which the backend calls from `spawn_blocking`.

## Parity rules

Until the gates pass, behaviour is the Python's, observable detail by
observable detail:

- **HTTP**: status codes, response bodies (keys, value types: `1` is not
  `1.0`, `null` is not a missing key), the headers clients read
  (`Content-Type`, CORS, `Last-Modified`, `Retry-After`), and tornado's
  routing (first matching regex in the sorted route list, full match).
- **Socket.IO**: event names, payload shapes (a JSON *string* where Python
  emits `json.dumps(...)`, an object where it emits a dict), who receives them
  (`skip_sid`, `to`).
- **UART**: the exact bytes written to the ESP32, including `str(float)`
  formatting (`pyfmt::repr_f64`) and `json.dumps` spacing (`pyfmt::dumps_default`).
- **Files**: paths and formats under `/meticulous-user`: config YAML, profile
  JSON (`json.dump(..., indent=4)`, ASCII only), shot files (zstd JSON), the
  SQLite schema and its alembic revision ids. A machine can roll back to the
  Python image after an OTA, so what Rust writes Python must read.

Log wording is not a contract, except the redaction tokens and the messages
listed in the boot harness. ERROR records mean the same as in Python: a boot
logs nothing at ERROR.

A Python bug that a client could depend on is kept and marked with a comment
starting `PARITY:`. Fixing it is a separate, later change.

## Tests

Every Python test in `tests/` protects a behaviour; the Rust code keeps that
protection:

- logic (parsing, conversion, validation, state machines): a Rust unit test in
  the crate, with the Python test's cases and expected values;
- observable behaviour (HTTP, Socket.IO, UART bytes, files): a black-box test
  under `tests/` that runs the binary.

Name the Python test a Rust test came from in its doc comment. Expected values
come from the Python code or its tests, never from what the Rust code returns.

## The backend crate

- **Services** (`crates/backend/src/<name>.rs`) replace the Python's
  class-level singletons. A service is a struct built in `App::new`
  (`app.rs`) that holds what it always needs (`Arc<ConfigStore>`, `Arc<Sio>`,
  `Arc<NotificationManager>`…); methods that need other services take
  `app: &App`. Start-up work that touches hardware or spawns tasks goes in
  `start()` in `main.rs`, in the order `backend.main()` does it.
- **Routes** live in `crates/backend/src/api/<python module>.rs`, each with a
  `routes() -> Vec<Route>` registered in `api/mod.rs`. Patterns are the
  Python's `register_handler` patterns, relative to `/api/v1`. A handler is
  `async fn(Arc<App>, Request) -> Reply`:
  `Reply::base().json(v)` is tornado's `self.write(dict)`,
  `Reply::base().dumps(v)` is `self.write(json.dumps(v))` (the content type
  stays `application/json` without the charset), `.status(code)` is
  `set_status`, and `Reply::error(Kind::Base, 500, &req)` is what tornado
  sends for an exception the Python handler does not catch.
  `api::local_only(&req)` is `LocalAccessHandler`.
- **Blocking work** (files, SQLite, zstd, subprocesses, D-Bus calls) runs in
  `tokio::task::spawn_blocking`; the UART has its own threads.
- **The host**: commands are run with `std::process::Command` with the
  Python's binaries and arguments (the boot harness stubs them on `PATH`);
  D-Bus through `zbus`; GPIO through `gpiocdev`. When
  `app.env.hooks.fake_hardware` is set, leave GPIO, I2C, BlueZ, systemd, the
  backlight and audio alone, as `tests/boot/boot_harness.py` does.
- **Clients**: `app.sio.emit(event, &value)` (`emit_to`, `emit_skip`),
  `app.notifications.add(Notification::new(..))`.
- **Settings**: `app.config.get(section, key)` and friends, `set`, `save`;
  key names in `config::keys`.
- **Tests**: `crate::testing::app()` builds an `App` on temporary
  directories; `testing::call(&app, Method::GET, "/api/v1/...", body)`
  sends a request through the real route table, like the Python suite's
  `AsyncHTTPTestCase`.

## Packaging

- **Data files**: the .deb installs the read-only data under
  `/opt/meticulous-backend` (`tests/package/verify_package.py` checks every
  tracked one ships). The small files the backend parses
  (`simple_profile.json`, the default sound theme's `config.json`, the default
  images' `accent_colors.json`) are also built into the binary
  (`crates/backend/src/bundled.rs`): a file on disk wins, a missing one falls
  back to the built-in copy with a WARNING. The sounds and the default
  profile images stay files only.
- **Debug info**: the release profile keeps debug info without types
  (`debug = "limited"`); `Dockerfile.deb` and
  `packaging/build-deb-cross.sh` split them into
  `meticulous-backend_<version>_<arch>.debug` next to the .deb and ship the
  binary with its symbol table only. Sentry symbolicates a crash report with
  that file once it is uploaded (`sentry-cli debug-files upload`, see
  `.github/workflows/package.yml`); a binary built elsewhere needs its own.

## Conventions

- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, no `unwrap()` in
  non-test code (`expect()` only for invariants, with the reason).
- Errors with `thiserror` in libraries; no panics on input from the network,
  the UART or files.
- No global mutable state: services hang off `App` (`crates/backend/src/app.rs`)
  and are passed in. Locks are short and never held across `.await`.
- JSON values that leave the process keep Python's key order
  (`serde_json` with `preserve_order`) and number types.
- Comments say why, not what; match the density of the code around them.
