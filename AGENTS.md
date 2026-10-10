# Working in meticulous-backend

Python backend of the Meticulous espresso machine: tornado HTTP API and
python-socketio for the Dial and the mobile app, SQLAlchemy/alembic history,
and the UART link to the ESP32 firmware. It ships as a .deb that
meticulous-machine installs in the image. Pull requests target `beta`; fixes
reach `stable` and `nightly` through the backport flow.

Every pull request runs the gates below. This file says what each one protects,
when a test is worth adding and where it goes. Tests that need the firmware or
the Dial together with the backend live in the private
MeticulousHome/meticulous-integration repository; its `ARCHITECTURE.md` describes
the validation levels across repositories.

## The gates

| Check | Protects | Reproduce locally |
|---|---|---|
| `lint` | flake8 and black | `uv run flake8 . && uv run black --check .` |
| `test` | the unit and handler tests | `uv run pytest` |
| `coverage` | total coverage stays above `fail_under`; on a PR, 80% of the changed lines are covered | `uv run pytest --cov --cov-report=xml tests log_redactor/tests/test_redaction_contract.py` then `uv run diff-cover coverage.xml --compare-branch=origin/beta` |
| `test-random-order` | no test passes only because another ran first | `uv run --with pytest-randomly pytest -p randomly --randomly-seed=<seed from the log>` |
| `typecheck` | no new mypy error (the existing ones are in `tests/static/mypy-baseline.txt`) | `uv run --with mypy==1.18.2 python tests/static/mypy_ratchet.py` |
| `client-contract` | everything meticulous-typescript-api calls, subscribes to and reads still exists | `python3 tests/contracts/check_ts_client.py --client <meticulous-typescript-api checkout>` |
| `redactor-pin` | the `log_redactor` submodule points at a commit on its main | |
| `deb (arm64)`, `deb (amd64)` | the .deb ships every module and data file, installs on Debian bookworm, and the installed backend boots twice with the hardware faked: every read-only GET below 500, the Socket.IO contract, the UART greeting, the DB migrated, nothing at ERROR | `tests/package/verify_package.py`, `tests/package/install_and_boot.sh` (see the Package workflow) |
| `integration/firmware` | the backend against the real firmware (identity, status stream, an espresso shot) | runs in meticulous-integration; the status links to the run |

Never weaken a gate to get green: no lower `fail_under`, no new entry in the mypy
baseline, no skipped test, no widened assert.

## Tests: when to add one

### The three questions

A new test answers all three, in its docstring (or its file's docstring) and in
the PR description.

1. **Which regression would reach a customer, the Dial or the app without it?**
   Name the behaviour ("loading a profile while brewing sends nothing to the
   ESP32"), not the code it runs ("covers LoadProfileHandler.post").
2. **Where does its expected value come from?** One of:
   - a constant or branch in the backend, named in the assertion (`409` and
     `MACHINE_BUSY` from `api/profiles.py`);
   - a published contract: the snapshots in `tests/contracts/`, the TypeScript
     client, the UART protocol and NVS keys the firmware owns, the profile schema;
   - the database schema that `alembic/versions` builds.

   A value chosen because it makes the test pass is not a source.
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
  contract: the log redaction tokens (`log_redactor/tests/test_redaction_contract.py`)
  and the UART fields are.

### Shape

- **Through the real handler.** `tornado.testing.AsyncHTTPTestCase` with the
  pattern exactly as `API.register_handler` registers it (`tests/test_api_*.py`).
- **Replace only hardware and the OS, at the boundary**: the ESP32 serial port,
  GPIO (`gpiod`), D-Bus (`pydbus`), `subprocess`, NetworkManager. `tests/conftest.py`
  stubs the machine-only packages and drops every Sentry DSN; tests never touch
  the network.
- **Temporary directories for every path** the code writes, and restore any
  global you change (`MeticulousConfig`, class attributes of `Machine` or the
  managers): the suite runs shuffled.
- **A failing test is a finding.** If the fix does not belong in the same change,
  mark it `@pytest.mark.xfail(strict=True, reason="backend bug: <file>:<line> ...")`.
  Fixing the bug then fails the test until the marker goes. Never skip, retry or
  loosen it.
- **Every test file opens with a docstring** saying what it protects.

## Changing a contract

| You change | Then |
|---|---|
| A route, its methods, a Socket.IO event, a payload key, a default setting | `tests/test_api_contracts.py` fails with a diff. Regenerate with `UPDATE_CONTRACTS=1 uv run pytest tests/test_api_contracts.py`, commit the snapshot, and say in the PR which client (Dial, app, meticulous-typescript-api) must follow. Removing something the TypeScript client uses fails `client-contract`. |
| The database schema | A new alembic revision, `DB_VERSION_REQUIRED` bumped, `database_models.py` kept identical (`tests/test_db_migrations.py` compares them), and a working `downgrade()`: an OTA rollback runs it. |
| The config file | New keys get a default in `DefaultConfiguration_V1`; a renamed or retyped key gets a migration in `MeticulousConfigDict.load()` (`tests/test_config_upgrades.py`). |
| Startup or a new init that touches hardware | Fake its boundary in `tests/boot/boot_harness.py`, or the Package boot fails. |
| A new module or data directory | Add it to `Dockerfile.deb`; `tests/package/verify_package.py` fails on a tracked file that is not packaged, unless `NOT_PACKAGED` says why. |
| A new host tool the backend runs | Add its package to `Depends:` in `debian/DEBIAN/control`; the image must not provide it by accident. |
| The UART protocol | The firmware owns it: change both sides together and check with the integration status. |
| Type errors | Do not add any. When you fix some, shrink the baseline: `tests/static/mypy_ratchet.py --update`. |

## Where a test goes

| You changed | Look at |
|---|---|
| `api/*.py` | `tests/test_api_<module>.py`, `tests/test_api_contracts.py` |
| `machine.py`, `esp_serial/` | `tests/test_machine_*.py`, `tests/test_profile_uart_payload.py`, the integration status |
| `profiles.py`, `profile_preprocessor.py`, `simple_profile.json`, `manual_*.py` | `tests/test_api_profiles.py`, `tests/test_profile_*.py`, `tests/test_bundled_profiles.py`, `tests/test_manual_mode.py` |
| `database_models.py`, `alembic/`, `db_migration_updater.py` | `tests/test_db_migrations.py` |
| `shot_manager.py`, `shot_database.py`, `shot_debug_manager.py` | `tests/test_shot_*.py`, `tests/test_api_history.py` |
| `config.py`, `settings_validation.py` | `tests/test_config*.py`, `tests/test_settings_validation.py`, `tests/contracts/settings_defaults.json` |
| `wifi.py`, `api/wifi.py` | `tests/test_wifi_*.py`, `tests/test_api_wifi.py` |
| `back.py`, `backend.py`, any `init()` | `tests/boot/boot_harness.py` (Package workflow) |
| `Dockerfile.deb`, `debian/` | `tests/package/` (Package workflow) |

## This repository is public

- Nothing that builds, runs or logs the firmware belongs here: it goes to
  meticulous-integration.
- Details of an unfixed vulnerability stay out of code, tests, commit messages and
  PR descriptions until the fix ships. Report it privately.
- No real serial numbers, SSIDs, passwords or keys in fixtures.

## Before opening a PR

```bash
uv sync --group dev
uv run flake8 . && uv run black --check .
uv run pytest
uv run --with mypy==1.18.2 python tests/static/mypy_ratchet.py
```

In the PR description, list each new test with the regression it prevents and
the bug it catches.
