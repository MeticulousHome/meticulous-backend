# The backend emulated, for development: docker-compose.yml (run_docker.sh)
# and the image build_docker.yml publishes. It builds the release binary like
# Dockerfile.deb (the same stages, so they share the cached layers) and runs
# it with run_emulated.sh. Compose mounts the checkout on /app; without the
# mount the image runs on the copies below.

FROM debian:bookworm AS toolchain
RUN apt-get update -y && apt-get install -y --no-install-recommends \
        ca-certificates curl build-essential pkg-config git \
    && rm -rf /var/lib/apt/lists/*
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
WORKDIR /build
COPY rust-toolchain.toml ./
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none \
    && rustup toolchain install \
    && cargo install cargo-chef --locked --version 0.1.78

FROM toolchain AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM toolchain AS builder
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json -p meticulous-backend
COPY . .
RUN cargo build --release --locked -p meticulous-backend \
    && cp target/release/meticulous-backend /meticulous-backend

FROM debian:bookworm
# dbus and socat for start_container.sh (its buses, the published port);
# gst-play-1.0 is the sound player.
RUN apt-get update -y && apt-get install -y --no-install-recommends \
        ca-certificates dbus gstreamer1.0-plugins-base-apps socat \
    && rm -rf /var/lib/apt/lists/* \
    && mkdir -p /run/dbus
COPY --from=builder /meticulous-backend /usr/bin/meticulous-backend
ENV METICULOUS_BACKEND_BIN=/usr/bin/meticulous-backend

WORKDIR /app
COPY run_emulated.sh start_container.sh UI_timezones.json simple_profile.json ./
COPY sounds/ sounds/
COPY images/default/ images/default/
COPY UI_timezones.json /usr/share/zoneinfo/

EXPOSE 8080
CMD ["/bin/bash", "/app/start_container.sh"]
