#!/bin/bash

DBUS_SYSTEM_BUS_ADDRESS=$(dbus-daemon --fork --config-file=/usr/share/dbus-1/system.conf --print-address)
export DBUS_SYSTEM_BUS_ADDRESS

DBUS_SESSION_BUS_ADDRESS=$(dbus-daemon --fork --config-file=/usr/share/dbus-1/session.conf --print-address)
export DBUS_SESSION_BUS_ADDRESS 

/opt/venv/bin/python3 /app/back.py
