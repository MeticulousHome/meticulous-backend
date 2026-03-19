#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERSION=$(grep '^version' "$SCRIPT_DIR/pyproject.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
ARCH="${1:-arm64}"
OUTPUT_DIR="${2:-$SCRIPT_DIR/out}"

echo "Building meticulous-backend ${VERSION} for ${ARCH}"

docker build \
    -f "$SCRIPT_DIR/Dockerfile.deb" \
    --platform "linux/$ARCH" \
    --build-arg VERSION="$VERSION" \
    --build-arg ARCH="$ARCH" \
    --output "type=local,dest=$OUTPUT_DIR" \
    "$SCRIPT_DIR"

echo "Built: $OUTPUT_DIR/meticulous-backend_${VERSION}_${ARCH}.deb"
