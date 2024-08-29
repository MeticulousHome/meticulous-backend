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

if [[ "$@" == *"--memory"* ]]; then
    if [ python3 -m memray ]; then
        python3 -m pip install memray
    fi
    profiling_file="memory_profiling_$(date -Iseconds).bin"
    python3 -m memray run -o ${profiling_file} back.py
    python3 -m memray flamegraph ${profiling_file}
    python3 -m memray summary ${profiling_file}
else
    python3 back.py
fi
