# meticulous-backend

## Introduction

This repository is used to run the backend of **meticulous**.
It handles ESP32 serial communication, shot management, profiles, sounds, OTA updates, and exposes a REST + Socket.IO API.

The backend is written in Rust: a Cargo workspace whose `meticulous-backend` binary is what the `.deb` ships. It is a drop-in replacement for the Python backend it replaced; the Python sources are in the history up to beta `f403168`, the commit CI's `rollback-compat` job checks Rust-written histories against. [`docs/rust/ARCHITECTURE.md`](docs/rust/ARCHITECTURE.md) describes the crates, where things live and the parity rules.

## Dependencies

Install Rust with [rustup](https://rustup.rs). `rust-toolchain.toml` pins the channel and the components (rustfmt, clippy, the `aarch64-unknown-linux-gnu` target), and rustup installs them on the first `cargo` command.

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
git submodule update --init
```

The submodules: `images/default` ships in the package, the profile and redactor crates' tests read `profile_schema` and `log_redactor`.

### Adding or updating dependencies

```bash
# Add a dependency to a crate
cargo add -p meticulous-backend <crate>

# Update the lock file
cargo update -p <crate>
```

`Cargo.lock` is committed and CI builds with `--locked`.

## Code style

- Formatter: **rustfmt** (`cargo fmt --all`)
- Linter: **clippy**, warnings are errors. The workspace denies `unsafe`, `dbg!`, `todo!` and `unwrap()` (allowed in tests by `clippy.toml`).
- The Python that stays (the test harnesses under `tests/`): **black** (line-length 96).
- Pre-commit hooks are configured in `.pre-commit-config.yaml`.

The conventions (errors, locks, comments, no global state) are in `docs/rust/ARCHITECTURE.md`.

## Backend: For Development

To allow developers to run the backend without a physical coffee machine, the ESP32 can be emulated: the backend replays a recorded shot on a pseudo-terminal.

### Using Docker

```bash
./run_docker.sh

# or, in the foreground
docker compose run --build -p 8080:8080 backend
```

The image (`Dockerfile`) builds the release binary and runs it with `run_emulated.sh`. Compose mounts the checkout on `/app`, so the config, profiles and history land in `./config`, `./profiles` and `./history`. The backend only listens on `127.0.0.1` (on a machine nginx is in front of it); `start_container.sh` forwards the container's address to it so the published port answers.

### Running Directly on Linux

```bash
./run_emulated.sh

# on another port
PORT=8081 ./run_emulated.sh
```

This builds the debug binary with cargo and runs it with `BACKEND=emulation`, its data under the checkout. `--memory` runs it under [heaptrack](https://github.com/KDE/heaptrack).

You can interact with the backend using the command line interface. For instance, you can enter the commands `l` and `r` to move the dial left or right, respectively, `e` to push it, and `start`, `stop`, `purge`, `home`, `tare` or `reset` to act on the machine (`crates/backend/src/events.rs`).

The backend reads the same environment variables the Python did (`crates/backend/src/env.rs`): `BACKEND` (`FIKA` by default, `USB`, `EMULATOR`), `PORT`, the data paths (`CONFIG_PATH`, `HISTORY_PATH`, `PROFILE_PATH`...), `EMULATION_SPEED`, `LOGLEVEL`, `SENTRY`. Off a machine, `METICULOUS_FAKE_HARDWARE=1` leaves GPIO, BlueZ, systemd and audio alone and `METICULOUS_ESP_UART` points the UART at a serial device (fika-sil's pty). Sentry reports to production with `BACKEND=FIKA` or `SENTRY=1`, unless the hardware is faked: keep both off on a workstation.

## Testing

```bash
./run_tests.sh
```

It runs CI's lint and test gates: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and black on the Python harnesses. On macOS it runs them inside Docker, in the `Dockerfile`'s toolchain stage.

```bash
# One crate; the tests whose path matches a filter
cargo test -p history
cargo test -p meticulous-backend profiles

# Regenerate the contract snapshots in tests/contracts/ after an intended change
UPDATE_CONTRACTS=1 cargo test -p meticulous-backend contract
```

`AGENTS.md` lists every CI gate, when a test is worth adding and where it goes.

## Database Migrations Guide

The history database keeps the alembic schema and revision ids of the Python backend, so a machine that rolls back to a Python image after an OTA can carry on from what Rust left. `crates/history` brings the database to `DB_VERSION_REQUIRED` at every boot, upgrading or downgrading along its revision chain.

### Making Database Changes

1. **Add a revision** to `REVISIONS` in `crates/history/src/migrate/revisions.rs`, with its `upgrade` and `downgrade` operations. The `CREATE TABLE` text is part of the on-disk contract: SQLite stores it.
2. **Add its alembic script** under `crates/history/alembic_scripts/`, with a working `downgrade()`. The backend copies the scripts to `USER_DB_MIGRATION_DIR`, where an older Python image finds the one it needs to downgrade.
3. **Bump `DB_VERSION_REQUIRED`** in `crates/history/src/migrate/mod.rs` and in `tests/boot/boot_harness.py`.
4. **Test it** in `crates/history/tests/migrations.rs`: up from every older revision, and back down.

### Rolling Back Changes

Set `DB_VERSION_REQUIRED` to an earlier revision: at the next boot the backend downgrades the database to it.

## Packaging and Deploying

```bash
# The .deb for a machine, with buildx (arm64 on an amd64 host goes through QEMU)
./build-deb.sh arm64 out/
```

`Dockerfile.deb` builds the binary natively for the target and assembles the package: `/usr/bin/meticulous-backend`, `met-config`, the data under `/opt/meticulous-backend`, and the paths the machine image still uses from the Python package.

To try a build on a machine from an amd64 workstation, cross-compile instead (`packaging/Dockerfile.cross-arm64`):

```bash
docker build -f packaging/Dockerfile.cross-arm64 -t meticulous-cross .
docker run --rm -v "$PWD:/src" -v meticulous-cross-target:/target \
    -v meticulous-cross-registry:/usr/local/cargo/registry \
    meticulous-cross packaging/build-deb-cross.sh
```

This writes `out/meticulous-backend_<version>_arm64.deb`. Copy it to the machine and install it as root:

```bash
dpkg -i meticulous-backend_<version>_arm64.deb
systemctl restart meticulous-backend
journalctl -u meticulous-backend -f
```

The package lives in the booted RAUC slot: the next OS update replaces it.
