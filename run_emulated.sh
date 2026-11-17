#!/bin/bash
# Run the backend with the ESP32 emulated (a replayed shot on a pty) and its
# data under this checkout. The binary is built with cargo, unless
# METICULOUS_BACKEND_BIN names one (the Docker image does).
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")"

export BACKEND=emulation
export CONFIG_PATH=./config
export LOG_PATH=./logs
export PROFILE_PATH=./profiles
export POUR_OVER_PROFILE_PATH=./pour-over-profiles
export HISTORY_PATH=./history
export DEBUG_HISTORY_PATH=./history/debug
export MOTOR_ENERGY_PATH=./history/energy
export UPDATE_PATH=/tmp/firmware
export PORT=${PORT:-8080}
export ZEROCONF_PORT=8080
export DEBUG=y
export USER_SOUNDS=./sounds
export DEFAULT_IMAGES=./images/default
export IMAGES_PATH=./images/profile-images
export TIMEZONE_JSON_FILE_PATH=./UI_timezones.json
export USER_DB_MIGRATION_DIR=./db-migrations
export ALARMS_PATH=./alarms
export REPORTS_DIR=./reports
# On a machine these are in /root and /meticulous-user. Without a writable
# redaction key every log record reads "redaction failed".
export REDACTION_KEY_PATH=./config/.redaction_key
export DEVICE_UUID_CACHE_PATH=./config/.device-identity/device-uuid
# The read-only data the package installs in /opt/meticulous-backend (system
# sounds, default images, simple_profile.json) comes from the checkout.
export METICULOUS_PACKAGE_ROOT=$PWD

if [ -z "${METICULOUS_BACKEND_BIN:-}" ]; then
    cargo build --locked -p meticulous-backend
    METICULOUS_BACKEND_BIN="${CARGO_TARGET_DIR:-target}/debug/meticulous-backend"
fi

if [[ "$*" == *"--memory"* ]]; then
    # Every allocation, for heaptrack_gui (flame graph) or heaptrack_print.
    exec heaptrack -o "memory_profiling_$(date -Iseconds)" "$METICULOUS_BACKEND_BIN"
else
    exec "$METICULOUS_BACKEND_BIN"
fi
