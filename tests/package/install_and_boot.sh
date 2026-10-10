#!/bin/bash
# Install the built .deb on a clean Debian bookworm (the machine image's base)
# and boot it twice with tests/boot/boot_harness.py: once as a fresh machine,
# once as a restart that finds its own config, profiles and history.
#
# Runs inside the container:
#   docker run --rm -v "$PWD:/src:ro" -v "$PWD/out:/deb:ro" -v "$PWD/boot-reports:/reports" \
#       debian:bookworm bash /src/tests/package/install_and_boot.sh
set -euo pipefail

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
# apt resolves the control file's Depends, so a dependency missing from the
# Debian archive fails here instead of on a machine.
apt-get install -y -qq --no-install-recommends /deb/*.deb >/dev/null

# The machine has a system bus and the backend connects to it at import time.
mkdir -p /run/dbus
dbus-daemon --system --fork

data_dir=$(mktemp -d)
for boot in 1 2; do
    echo "== boot ${boot}"
    /opt/meticulous-venv/bin/python3 /src/tests/boot/boot_harness.py \
        --package-root /opt/meticulous-backend \
        --data-dir "${data_dir}" \
        --report "/reports/boot-${boot}.json" \
        --boot-index "${boot}"
done
echo "installed package booted twice"
