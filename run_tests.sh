#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUN_COMMAND="pytest /app/tests/ -v"

if [ "$(uname)" = "Darwin" ]; then
    echo "macOS detected, running tests inside Docker..."
    docker compose -f "$SCRIPT_DIR/docker-compose.yml" run \
        --rm \
        --build \
        --no-deps \
        -v "$SCRIPT_DIR:/app" \
        backend \
        bash -c "$RUN_COMMAND"
else
    pip install -r "$SCRIPT_DIR/requirements-dev.txt"
    $RUN_COMMAND
fi
