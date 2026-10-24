#!/bin/bash

DBUS_SYSTEM_BUS_ADDRESS=$(dbus-daemon --fork --config-file=/usr/share/dbus-1/system.conf --print-address)
export DBUS_SYSTEM_BUS_ADDRESS

DBUS_SESSION_BUS_ADDRESS=$(dbus-daemon --fork --config-file=/usr/share/dbus-1/session.conf --print-address)
export DBUS_SESSION_BUS_ADDRESS

# The backend only listens on 127.0.0.1 (nginx fronts it on a machine):
# forward the container's own addresses to it, so a published port reaches it.
for address in $(hostname -I); do
    case "$address" in
        *:*) ;;
        *) socat "TCP-LISTEN:${PORT:-8080},bind=${address},fork,reuseaddr" "TCP:127.0.0.1:${PORT:-8080}" & ;;
    esac
done

pushd /app
./run_emulated.sh
