#!/bin/sh
export BACKEND=emulation
export CONFIG_PATH=./config
export LOG_PATH=./logs
export PROFILE_PATH=./profiles
export HISTORY_PATH=./shots
export DEBUG_HISTORY_PATH=./shots/debug
export UPDATE_PATH=/tmp/firmware
export PORT=8080
export DEBUG=y
export USER_SOUNDS=./sounds
python3 back.py
