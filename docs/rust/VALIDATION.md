# Validating the Rust backend

What was checked before the Python backend was removed, how, and what came
out. The Python numbers are beta `f403168` on the same hardware.

## Gates

| Where | What |
|---|---|
| `rust.yml` | rustfmt and clippy (`-D warnings`), every target compiles, 1195 tests, the same tests in random order, line coverage ≥ 65% and ≥ 80% of a PR's changed lines, `rollback-compat` (the Python backend of `f403168` reads and downgrades a history Rust wrote) |
| `package.yml` | the .deb for arm64 (native runner) and amd64 ships every runtime file, installs on a clean bookworm and boots three times with the hardware faked (fresh, restart, a package missing its data files), plus the split debug file |
| `build.yml` | the Python tooling lints clean, `client-contract` (meticulous-typescript-api only calls what the backend serves), `redactor-pin` |
| meticulous-integration (`ci/local.sh`) | backend × firmware against fika-sil built from EspressoFirmware beta: 3/3. The Dial's journeys (its beta web build in Chromium on the same stack): 27 passed, 2 XPASS(strict); the Python backend gives the same two XPASS, the Dial fixed those bursts after the markers were written |

## The Python suite

The 875 cases of `tests/` were inventoried against the Rust tests. Library
logic is ported in its crate; the handler suites (profiles, settings, sounds,
notifications, actions, machine, history, manual mode, Pour Over, base
handler, local-only policy, device UUID, debug shot log) are in
`crates/backend/src/ported/`, one test per case naming it. Porting them found
and fixed divergences from the Python: config saves did not emit `settings`;
JSON error messages and positions now follow CPython 3.11's decoder; sound
themes listed in the wrong order; `GET /smoke-validation` never saw the
machine; local-only routes answered 204/405 to a LAN client where tornado's
`prepare()` answers 403; older profiles on disk were not re-saved; debug
shot log records were dropped while the UART thread held the shot's lock;
`profileHover` turned client values into strings.

Strict xfails of the Python suite: a crash (an uncaught exception, a 500) is
fixed and marked `Deliberate fix:`; any other quirk is kept and marked
`PARITY:`.

## On a machine

VAR-SOM-MX8M-NANO (i.MX8M Nano, 2 GB), Debian 12, image `2026M1468-beta`,
ESP32-S3 firmware `0.2.24-469-gf8064c0`, the Rust .deb installed over the
Python one (`dpkg -i`), console on the debug UART, mains through a relay.

| Checked | Result |
|---|---|
| UART and GPIO | handshake, identity, version and mileage adopted; `status`/`sensors` follow the firmware's 100 ms frames |
| Firmware flashing (`POST /update/firmware`) | GPIO reset into the ROM bootloader, espflash stub at 921600 baud, 4 images written and hash-verified in 11.8 s (esptool: 12.2 s), back in the firmware 5.7 s later |
| Wi-Fi | config, scan list, QR; client → hotspot → client; forget and join again through the API (10 s, deep health verified) |
| BLE | advertisement with mode and IPv4 updates live (client and hotspot); GATT and Improv identical to a Python machine's; Wi-Fi provisioning over Improv answers with the machine's URLs in 2.7 s |
| mDNS | **found broken and fixed**: after a network change the services stayed announced with stale addresses (the restart raced NetworkManager's IPv4); they are now renewed once the addresses settle. Three hotspot/client round trips stay discoverable |
| Cold boot (relay) | serving with the ESP32 handshake done at 16.3 s from mains on (Python 27.0 s); the network itself is up at ~15.5 s either way |
| Upgrade over the Python package | `postinst` clears the bytecode the Python left (88 `.pyc`); `/opt/meticulous-venv` goes from 196 MB to a symlink |
| Memory soak | 1200 heavy requests (history listings, machine info, profiles, Socket.IO handshakes): RSS flat |

## Measurements

On the machine (median of five unless noted):

| | Python | Rust |
|---|---|---|
| Restart to first HTTP answer | 12.1 s | 5.8 s |
| Mains on to serving (two cold boots) | 27.5 / 26.4 s | 16.9 / 15.6 s |
| CPU while idle with the Dial connected (60 s) | 10.4 % | 1.8 % |
| RSS idle | 124 MB | 24 MB |
| RSS after 25 heavy history listings | — | 26 MB (107 MB with glibc's default allocator settings) |
| `GET /history?max_results=20` (2.3 MB) | 0.69 s | 0.11 s |
| `GET /machine` | 0.45 s | 0.44 s (NetworkManager's answer dominates) |
| Installed size | 9.4 MB + 196 MB venv | 32 MB binary (12.9 MB .deb) |

Against the real firmware on fika-sil in real time, same container, one
client (`docs/rust/benchmark`: 120 knob detents, 30 s idle, one shot):

| | Python | Rust |
|---|---|---|
| Knob detent to the client's `button` event, p50 / p90 / p99 | 53.3 / 54.5 / 56.2 ms | 8.3 / 9.2 / 30.8 ms |
| Backend CPU idle / during a shot | 0.83 % / 0.91 % | 0.40 % / 0.44 % |

## Open

- meticulous-machine builds the production .deb itself (`build-deb.sh`):
  its pipeline needs its own `sentry-cli debug-files upload` for symbolicated
  crash reports.
- The TypeScript client's types are narrower than the payloads in places
  (`docs/rust/client-types.md`).
- meticulous-integration's two Dial burst markers are stale (XPASS on both
  backends).
