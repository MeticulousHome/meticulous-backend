# Emulated shot end-to-end test

The backend E2E test starts the real REST and Socket.IO application with the
existing ESP serial emulator. It uses a fresh runtime directory, loads a
default profile, sends `action/start`, replays the recorded espresso telemetry,
and verifies that the completed shot is persisted with the selected profile.

`HEADLESS_EMULATION=true` is valid only with `BACKEND=emulation`. It disables
machine-only D-Bus, USB, BLE, NetworkManager, Zeroconf, image metadata, SSH,
system service, audio, host timezone, disk-flashing and Sentry startup.
Production defaults are unchanged. `BIND_ADDRESS` remains `127.0.0.1` unless a
container runner explicitly selects another address.

The test proves:

- the backend can boot without physical machine services;
- profile save and load work through the public API;
- the backend sends `action,start` to the serial emulator;
- recorded ESP telemetry enters the shot manager; and
- the completed shot and selected profile are stored in history.

It does not prove physical sensor accuracy, actuator safety, firmware timing or
Dial rendering. Those belong to hardware-in-the-loop and Dial-owned suites.

The GitHub workflow runs on every backend pull request, pushes to `nightly`, a
daily schedule and manual dispatch. It uses a new runtime directory for every
job, so state reset is deterministic without adding a production API.

To run only the client assertions against an already-running headless backend:

```bash
E2E_BACKEND_URL=http://127.0.0.1:18080 \
  python -m unittest tests.e2e.test_emulated_shot -v
```
