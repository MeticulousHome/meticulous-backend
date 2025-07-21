#!/bin/bash
export BACKEND=emulation
export CONFIG_PATH=./config
export LOG_PATH=./logs
export PROFILE_PATH=./profiles
export HISTORY_PATH=./history
export DEBUG_HISTORY_PATH=./history/debug
export UPDATE_PATH=/tmp/firmware
export PORT=8080
export DEBUG=y
export USER_SOUNDS=./sounds
export DEFAULT_IMAGES=./images/default
export IMAGES_PATH=./images/profile-images
export DEFAULT_PROFILES=./default_profiles
export TIMEZONE_JSON_FILE_PATH=./UI_timezones.json
export USER_DB_MIGRATION_DIR=./db-migrations

PYTHON=python3
if [[ -e .venv ]]; then
	PYTHON=.venv/bin/python3
fi

if [[ "$@" == *"--memory"* ]]; then
    if [ $PYTHON -m memray ]; then
        $PYTHON -m pip install memray
    fi
    profiling_file="memory_profiling_$(date -Iseconds).bin"
    $PYTHON -m memray run -o ${profiling_file} back.py
    $PYTHON -m memray flamegraph ${profiling_file}
    $PYTHON -m memray summary ${profiling_file}
else
    $PYTHON back.py
fi
