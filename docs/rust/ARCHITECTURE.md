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
crates/backend/            the `meticulous-backend` binary: HTTP, Socket.IO, the machine, storage, OS
tests/                     black-box tests that drive the binary over HTTP, Socket.IO and a pty
```

Pure crates have no IO, no globals, no async and no clocks: time comes in as
an argument. Everything that touches the network, the disk, D-Bus, GPIO or the
UART lives in `crates/backend`.

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
