#!/bin/sh
export BACKEND=emulation
export CONFIG_PATH=./config
export LOG_PATH=./logs
export PROFILE_PATH=./profiles
export HISTORY_PATH=./history/shots
export DEBUG_HISTORY_PATH=./history/debug
export UPDATE_PATH=/tmp/firmware
export PORT=8080
export DEBUG=y
export USER_SOUNDS=./sounds
export DEFAULT_IMAGES=./images/default
export IMAGES_PATH=./images/profile-images
export DEFAULT_PROFILES=./default_profiles
python3 back.py
