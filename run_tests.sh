#!/bin/bash
# The checks CI runs on every pull request (.github/workflows/rust.yml), and
# black on the Python harnesses that drive the binary from outside.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

if [ "$(uname)" = "Darwin" ]; then
    echo "macOS detected, running tests inside Docker..."
    # The Dockerfile's toolchain stage; Linux build artifacts and the crate
    # registry stay in volumes, apart from the checkout's target/.
    docker build -q --target toolchain -t meticulous-backend-toolchain "$SCRIPT_DIR" >/dev/null
    exec docker run --rm \
        -v "$SCRIPT_DIR:/build" \
        -v meticulous-backend-target:/build/target \
        -v meticulous-backend-registry:/usr/local/cargo/registry \
        meticulous-backend-toolchain ./run_tests.sh
fi

cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked

harness=()
for path in tests/boot tests/package tests/contracts/check_ts_client.py; do
    if [ -e "$path" ]; then
        harness+=("$path")
    fi
done
if [ ${#harness[@]} -gt 0 ]; then
    # CI's black: 26.3.1, line length 96.
    if command -v black >/dev/null; then
        black --check --line-length 96 "${harness[@]}"
    elif command -v uvx >/dev/null; then
        uvx --from black==26.3.1 black --check --line-length 96 "${harness[@]}"
    else
        echo "black not found: skipped the check of ${harness[*]}" >&2
    fi
fi
