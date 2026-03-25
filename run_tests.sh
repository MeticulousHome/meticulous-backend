#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RUN_COMMAND="pytest tests/ -v"

if [ "$(uname)" = "Darwin" ]; then
    echo "macOS detected, running tests inside Docker..."
    docker compose -f "$SCRIPT_DIR/docker-compose.yml" run \
        --rm \
        --build \
        --no-deps \
        -v "$SCRIPT_DIR:/app" \
        backend \
        bash -c "cd /app && black . && flake8 && $RUN_COMMAND"
else
    uv run $RUN_COMMAND
fi
