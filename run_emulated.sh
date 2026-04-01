#!/bin/bash
export BACKEND=emulation
export CONFIG_PATH=./config
export LOG_PATH=./logs
export PROFILE_PATH=./profiles
export HISTORY_PATH=./history
export DEBUG_HISTORY_PATH=./history/debug
export MOTOR_ENERGY_PATH=./history/energy
export UPDATE_PATH=/tmp/firmware
export PORT=8080
export ZEROCONF_PORT=8080
export DEBUG=y
export USER_SOUNDS=./sounds
export DEFAULT_IMAGES=./images/default
export IMAGES_PATH=./images/profile-images
export DEFAULT_PROFILES=./default_profiles
export TIMEZONE_JSON_FILE_PATH=./UI_timezones.json
export USER_DB_MIGRATION_DIR=./db-migrations
export ALARMS_PATH=./alarms

uv sync --group dev --group machine

if [[ "$@" == *"--memory"* ]]; then
    uv run --with memray python3 -m memray run -o "memory_profiling_$(date -Iseconds).bin" back.py
    uv run --with memray python3 -m memray flamegraph "memory_profiling_$(date -Iseconds).bin"
    uv run --with memray python3 -m memray summary "memory_profiling_$(date -Iseconds).bin"
else
    uv run python3 back.py
fi
