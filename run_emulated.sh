#!/bin/sh
export BACKEND=emulation
export CONFIG_PATH=./config
export LOG_PATH=./logs
export PROFILE_PATH=./profiles
export HISTORY_PATH=./shots
export UPDATE_PATH=/tmp/firmware
export PORT=8080
python3 back.py
