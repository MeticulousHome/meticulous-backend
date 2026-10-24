# Working in meticulous-backend

Rust backend of the Meticulous espresso machine: the HTTP API and Socket.IO
for the Dial and the mobile app, the SQLite history, and the UART link to the
ESP32 firmware. It ships as a .deb that meticulous-machine installs in the
image, a drop-in replacement for the Python backend: the same routes, events,
UART bytes and files. The Python sources stay in the tree as the reference
until they are removed. Read [`docs/rust/ARCHITECTURE.md`](docs/rust/ARCHITECTURE.md)
first: the crates, the backend's services and routes, the parity rules and the
conventions. Pull requests target `beta`; fixes reach `stable` and `nightly`
through the backport flow.

Every pull request runs the gates below. This file says what each one protects,
when a test is worth adding and where it goes. How these gates fit the checks
across repositories, up to the image, is in meticulous-machine's
[`docs/testing/validation-architecture.md`](https://github.com/MeticulousHome/meticulous-machine/blob/beta/docs/testing/validation-architecture.md).
In short: this repository tests itself, owns and proves the HTTP and Socket.IO
contract, and is tested against the real firmware of the same branch on every
pull request (beta with beta, nightly with nightly, stable with stable).

## The gates

| Check | Protects | Reproduce locally |
|---|---|---|
| `Rust / lint` | rustfmt and clippy, warnings are errors | `cargo fmt --all --check && cargo clippy --workspace --all-targets --locked -- -D warnings` |
| `Rust / typecheck` | every crate and test compiles | `cargo check --workspace --all-targets --locked` |
| `Rust / test` | the unit tests in each crate and the black-box tests in `crates/*/tests` | `cargo test --workspace --locked` |
| `Rust / coverage` | 65% of the lines stay covered; on a PR, 80% of the changed lines are | `cargo llvm-cov --workspace --locked --cobertura --output-path coverage.xml --fail-under-lines 65` then `diff-cover coverage.xml --compare-branch=origin/beta --fail-under=80` |
| `Rust / test-random-order` | no test passes only because another ran first | `cargo +nightly test --workspace -- -Z unstable-options --shuffle --shuffle-seed <seed from the log>` |
| `client-contract` | everything meticulous-typescript-api calls, subscribes to and reads still exists | `python3 tests/contracts/check_ts_client.py --client <meticulous-typescript-api checkout>` |
| `redactor-pin` | the `log_redactor` submodule points at a commit on its main | |
| `deb (arm64)`, `deb (amd64)` | the .deb ships the binary and every data file, installs on Debian bookworm, and the installed backend boots twice with the hardware faked: every read-only GET below 500, the Socket.IO contract, the UART greeting, the DB migrated, nothing at ERROR | `tests/package/verify_package.py`, `tests/package/install_and_boot.sh` (see the Package workflow) |
| `integration/firmware` | the backend against the real firmware of the same branch, the firmware's own fika-sil simulator (identity and version, the status stream, an espresso shot); a firmware branch with this PR's name is used when it exists | runs in meticulous-integration (private); the status links to the run. Locally: its `ci/local.sh` |

The Rust gates are `.github/workflows/rust.yml`; the package, `package.yml`.
`build.yml` still runs the Python suite against the Python sources (lint, test,
coverage, mypy) and goes with them; its `client-contract` and `redactor-pin`
stay. `./run_tests.sh` runs lint and test, and black on the Python harnesses.

Never weaken a gate to get green: no lower coverage threshold, no `#[ignore]`,
no `#[allow(clippy::...)]` to silence a finding, no widened assert.

## Parity

Behaviour is the Python's, observable detail by observable detail
(`docs/rust/ARCHITECTURE.md`, "Parity rules"): status codes and bodies, key
order and number types, Socket.IO payloads and recipients, UART bytes, files
under `/meticulous-user`. A machine can roll back to a Python image after an
OTA, so what Rust writes Python must read.

- A Python bug a client could depend on is kept and marked with a comment
  starting `PARITY:` that says what the Python does. Fixing it is a separate,
  later change that names the client affected.
- A Python bug no client can depend on (a 500 from an uncaught exception, a
  crash on malformed input) may be fixed in the port, marked with a comment
  starting `Deliberate fix:` that says what the Python did, and covered by a
  test of the new behaviour.

Anything else that differs from the Python is a bug in the port.

## Tests: when to add one

### The three questions

A new test answers all three, in its doc comment (or its file's `//!`) and in
the PR description.

1. **Which regression would reach a customer, the Dial or the app without it?**
   Name the behaviour ("loading a profile while brewing sends nothing to the
   ESP32"), not the code it runs ("covers profiles::load").
2. **Where does its expected value come from?** One of:
   - a constant or branch in the backend, named in the assertion (`409` and
     `"machine is busy"` from `crates/backend/src/api/profiles.rs`);
   - a published contract: the snapshots in `tests/contracts/`, the TypeScript
     client, the UART protocol and NVS keys the firmware owns, the profile schema;
   - the database schema that `crates/history/src/migrate/revisions.rs` builds;
   - for a ported test, the Python code or its test, named in the doc comment.

   A value chosen because it makes the test pass, or copied from what the Rust
   returns, is not a source.
3. **Which bug does it catch that the suite misses today?** Plant it (or revert
   your change) and run the suite: if it stays green and your test fails, the
   test earns its place. If an existing test already fails, extend that test.

### Add a test when

- You fix a bug: reproduce it first; the test fails before the fix and passes
  after it.
- You change behaviour a client sees: a status code, a response body, a
  Socket.IO event, bytes sent to the ESP32, a file or setting persisted.
- A new contract surface appears: a route, an event, a setting, a migration, a
  host tool the backend runs.

### Do not add a test

- For a refactor that keeps behaviour: the existing suite is the proof.
- To reach a coverage number.
- On implementation details (private names, log wording), unless the detail is a
  contract: the log redaction tokens (`crates/redactor/tests/redaction_contract.rs`)
  and the UART fields are.

### Shape

- **Through the real route table.** `testing::call(&app, Method::GET, "/api/v1/...", body)`
  on a `testing::app()` (`crates/backend/src/testing.rs`), with the pattern as
  the route registers it, like the Python suite's `AsyncHTTPTestCase`.
- **Replace only hardware and the OS, at the boundary**: the ESP32 UART, GPIO,
  D-Bus, the host commands, NetworkManager. Test apps have no Sentry client and
  tests never touch the network.
- **Temporary directories for every path** the code writes (`testing::app()`
  puts them all under one), and no global state: services hang off `App`, so
  the suite runs shuffled.
- **A failing test is a finding.** Fix the bug in the same change, or keep the
  Python's behaviour, assert it and mark it `PARITY:`. Never ignore, retry or
  loosen it.
- **Every test file opens with a `//!` comment** saying what it protects.

## Changing a contract

| You change | Then |
|---|---|
| A route, its methods, a Socket.IO event, a payload key, a default setting | `contract::tests::contracts_match_their_snapshots` fails with a diff. Regenerate with `UPDATE_CONTRACTS=1 cargo test -p meticulous-backend contract`, commit the snapshot, and say in the PR which client (Dial, app, meticulous-typescript-api) must follow. Removing something the TypeScript client uses fails `client-contract`. |
| The database schema | A new revision in `crates/history/src/migrate/revisions.rs` with a working downgrade, its alembic script in `crates/history/alembic_scripts/` (an older Python image runs its `downgrade()` after an OTA rollback), and `DB_VERSION_REQUIRED` bumped in `crates/history/src/migrate/mod.rs` and `tests/boot/boot_harness.py` (`crates/history/tests/migrations.rs`). |
| The config file | New keys get a default in `config::defaults()`; a renamed or retyped key gets a migration in the load-time `normalize()` (`crates/backend/src/config/mod.rs`). |
| Startup or a new init that touches hardware | Leave it alone under `app.env.hooks.fake_hardware` and fake its boundary in `tests/boot/boot_harness.py`, or the Package boot fails. |
| A data file the backend reads at run time | Add it to `Dockerfile.deb`, `packaging/build-deb-cross.sh` and the dev `Dockerfile`, and its pattern to `DATA_FILES` in `tests/package/verify_package.py`. |
| A new host tool the backend runs | Add its package to `Depends:` in `debian/DEBIAN/control`; the image must not provide it by accident. Stub it in the boot harness. |
| The UART protocol | The firmware owns it and describes it in its `contracts/uart/protocol.json` (`FIRMWARE_UART_CONTRACT=<file> cargo test -p esp --test firmware_contract` checks the backend against it). Change both sides in branches with the same name: integration tests each PR against the other. Keep the backend compatible with the previous firmware for one step: after an update it talks to the old firmware until it flashes the new one. |

## Where a test goes

| You changed | Look at |
|---|---|
| `crates/backend/src/api/*.rs` | the module's tests, `crates/backend/src/ported/test_api_<module>.rs`, the contract snapshot test |
| `crates/backend/src/machine/`, `crates/esp` | `crates/esp/tests/`, the machine module's tests, the integration status |
| `crates/profile`, `profiles.rs`, `simple_profile.json`, `manual_mode.rs` | `crates/profile/tests/`, `ported/test_api_profiles.rs`, `ported/test_manual_mode.rs` |
| `crates/history`, `shots.rs`, `debug_shots.rs` | `crates/history/tests/` (`python_written.rs` and `python_reads_rust.rs` for the files both ways) |
| `crates/backend/src/config/`, `api/settings.rs` | `ported/test_config.rs`, `ported/test_api_settings.rs`, `ported/test_settings_validation.rs`, `tests/contracts/settings_defaults.json` |
| `crates/backend/src/wifi/` | `wifi/tests.rs` |
| `crates/redactor` | `crates/redactor/tests/` |
| `main.rs`, any `init()` or `start()` | `crates/backend/tests/boot.rs`, `tests/boot/boot_harness.py` (Package workflow) |
| `Dockerfile.deb`, `debian/` | `tests/package/` (Package workflow) |

## This repository is public

- Nothing that builds, runs or logs the firmware belongs here: it goes to
  meticulous-integration.
- Details of an unfixed vulnerability stay out of code, tests, commit messages and
  PR descriptions until the fix ships. Report it privately.
- No real serial numbers, SSIDs, passwords or keys in fixtures.

## Before opening a PR

```bash
./run_tests.sh
```

To try the change on a machine, build the arm64 .deb with
`packaging/build-deb-cross.sh` (README, "Packaging and Deploying").

In the PR description, list each new test with the regression it prevents and
the bug it catches.
