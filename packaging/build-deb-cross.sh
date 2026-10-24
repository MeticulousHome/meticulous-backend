#!/bin/bash
# Build the arm64 .deb inside packaging/Dockerfile.cross-arm64: the same
# package Dockerfile.deb assembles, from a cross-compiled binary. Writes
# out/meticulous-backend_<version>_arm64.deb and, next to it, the binary's
# debug file out/meticulous-backend_<version>_arm64.debug (see Dockerfile.deb).
set -euo pipefail
cd /src
version=$(grep '^version' pyproject.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')

cargo build --release --locked --target aarch64-unknown-linux-gnu -p meticulous-backend

built="$CARGO_TARGET_DIR/aarch64-unknown-linux-gnu/release/meticulous-backend"
split=$(mktemp -d)
aarch64-linux-gnu-objcopy --only-keep-debug "$built" "$split/meticulous-backend.debug"
aarch64-linux-gnu-objcopy --strip-debug --add-gnu-debuglink="$split/meticulous-backend.debug" \
    "$built" "$split/meticulous-backend"

pkg=$(mktemp -d)
cp -r debian/. "$pkg/"
VERSION="$version" ARCH=arm64 envsubst '${VERSION} ${ARCH}' < debian/DEBIAN/control > "$pkg/DEBIAN/control"
install -D -m 755 "$split/meticulous-backend" "$pkg/usr/bin/meticulous-backend"
chmod 755 "$pkg/DEBIAN/postinst" "$pkg/usr/bin/met-config"
mkdir -p "$pkg/opt/meticulous-venv/bin" "$pkg/opt/meticulous-backend/api/web_ui" \
    "$pkg/opt/meticulous-backend/images/default" "$pkg/opt/meticulous-backend/sounds"
ln -s /usr/bin/python3 "$pkg/opt/meticulous-venv/bin/python3"
cp packaging/met-config.py "$pkg/opt/meticulous-backend/met-config.py"
cp UI_timezones.json simple_profile.json "$pkg/opt/meticulous-backend/"
cp -r sounds/. "$pkg/opt/meticulous-backend/sounds/"
cp images/default/*.png images/default/accent_colors.json "$pkg/opt/meticulous-backend/images/default/"

mkdir -p out
dpkg-deb --build --root-owner-group "$pkg" "out/meticulous-backend_${version}_arm64.deb"
cp "$split/meticulous-backend.debug" "out/meticulous-backend_${version}_arm64.debug"
chown "$(stat -c %u:%g /src)" out "out/meticulous-backend_${version}_arm64.deb" \
    "out/meticulous-backend_${version}_arm64.debug"
